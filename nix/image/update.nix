# the files systemd-sysupdate installs a version from (ab-sysupdate.nix): the store and its verity
# partition cut out of the image, the uki, and SHA256SUMS over the three. a partition's uuid is in
# its file name. sysupdate gives the partition it writes that uuid, and the uki finds the store by it
{
  config,
  pkgs,
}:
let
  inherit (config.system.image) id version;
  inherit (config.system.build) intermediateImage uki;
  inherit (config.system.boot.loader) ukiFile;
  # the image before its esp is written. the partitions sysupdate reads are the same in both
  raw = "${intermediateImage}/${config.image.baseName}.raw";
in
pkgs.runCommand "${id}-update-${version}"
  {
    nativeBuildInputs = [
      pkgs.jq
      pkgs.zstd
    ];
    __structuredAttrs = true;
    # the uki names store paths, but these files are a system of their own and keep nothing alive
    unsafeDiscardReferences.out = true;
  }
  ''
    mkdir "$out"
    # one partition as <id>_<version>_<uuid>.<kind>.zst, from where repart put it in the image
    partition() {
      local type=$1 kind=$2 uuid offset size
      read -r uuid offset size < <(jq -r --arg type "$type" \
        '.[] | select(.type == $type) | "\(.uuid) \(.offset) \(.raw_size)"' \
        ${intermediateImage}/repart-output.json) || {
        echo "the image has no $type partition" >&2
        exit 1
      }
      echo "$kind: partition $uuid, $size bytes at $offset"
      dd if=${raw} iflag=skip_bytes,count_bytes skip="$offset" count="$size" bs=4M status=none |
        zstd -q -T"$NIX_BUILD_CORES" -12 -o "$out/${id}_${version}_$uuid.$kind.zst"
    }
    partition usr-x86-64-verity verity
    partition usr-x86-64 store
    cp ${uki}/${ukiFile} "$out/${id}_${version}.efi"

    cd "$out"
    sha256sum ${id}_${version}* > SHA256SUMS
    cat SHA256SUMS
  ''
