#!/usr/bin/env bash
# gives a drive or an image file slot b of the a/b layout. store a grows to its full 8G slot, and an
# empty 1G verity partition and an empty 8G store partition follow it, both labelled _empty, which is
# where systemd-sysupdate writes the next version. flash.sh and persist-image.sh run this before they
# add persist.
#
# Usage: sudo tools/add-slot-b.sh <image.raw|/dev/sdX>
# The image's store partition has to be its last one. An image file grows by what the slots need.
set -euo pipefail

target=${1:?image file or drive}
gib=$((1024 * 1024 * 1024))
store_size=$((8 * gib))
verity_size=$gib
# gpt types for /usr on x86-64 and its verity data, from the discoverable partitions specification
usr_type=8484680C-9521-48C6-9C11-B0720656F69E
verity_type=77FF5F63-E7B6-4633-ACF4-1565B864C0E6

[[ -f "$target" || -b "$target" ]] || { echo "not an image file or a drive: $target" >&2; exit 1; }
for tool in sfdisk sgdisk truncate blockdev; do
  command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 1; }
done

dump=$(sfdisk -d "$target")
if grep -q 'name="_empty"' <<<"$dump"; then
  echo "$target already has slot b" >&2
  exit 1
fi
sector=$(awk '$1 == "sector-size:" { print $2 }' <<<"$dump")
sector=${sector:-512}
mapfile -t stores < <(grep -E 'name="store_' <<<"$dump" || true)
if [[ ${#stores[@]} -ne 1 ]]; then
  echo "$target has ${#stores[@]} store partitions, expected the image's one" >&2
  exit 1
fi
node=${stores[0]%% :*}
num=${node##*[!0-9]}
start=$(sed -E 's/.*start= *([0-9]+).*/\1/' <<<"${stores[0]}")
last=$(grep -E ' : start=' <<<"$dump" | sed -E 's/.*start= *([0-9]+).*/\1/' | sort -n | tail -n1)
if [[ $start != "$last" ]]; then
  echo "the store partition is not the last one on $target, it cannot grow" >&2
  exit 1
fi

# store a at its full size, slot b, and room for alignment and the backup table
need=$((start * sector + store_size + verity_size + store_size + 64 * 1024 * 1024))
if [[ -f "$target" ]]; then
  if (($(stat -c %s "$target") < need)); then
    echo ">> Growing $target for two slots"
    truncate -s "$need" "$target"
  fi
elif (($(blockdev --getsize64 "$target") < need)); then
  echo "$target is too small for two 9G slots" >&2
  exit 1
fi

echo ">> Moving the GPT backup header to the end"
sgdisk -e "$target" >/dev/null
echo ">> Growing store a (partition $num) to 8G"
# only the end moves: start, type, uuid, label and flags stay, and so does the verity data in it
echo "size=$((store_size / sector))" |
  sfdisk --quiet --no-reread --no-tell-kernel --wipe-partitions never -N "$num" "$target" >/dev/null
echo ">> Adding slot b"
sgdisk -n "0:0:+$((verity_size / 1024 / 1024))M" -t "0:$verity_type" -c 0:_empty "$target" >/dev/null
sgdisk -n "0:0:+$((store_size / 1024 / 1024))M" -t "0:$usr_type" -c 0:_empty "$target" >/dev/null
