# totality: the read-only, verity-checked system image
#
# the image is slot a of the a/b layout: the esp with systemd-boot and a uki that counts its boots,
# the store's verity partition and the store. writing it to a drive adds slot b and persist behind it
# (eclipse-flash). ab-sysupdate.nix is the running side
{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:
let
  inherit (config.system.image) id version;
  systemdBoot = "${config.systemd.package}/lib/systemd/boot/efi/systemd-bootx64.efi";
in
{
  imports = [
    "${modulesPath}/image/repart.nix"
    "${modulesPath}/image/repart-verity-store.nix"
    ./persist.nix
    ./ab-sysupdate.nix
  ];

  boot.loader.grub.enable = false;
  boot.initrd.systemd.enable = true;

  # names the uki, the image file and the slot labels: eclipse_<version>, store_<version>
  system.image.id = "eclipse";
  system.image.version = "0.1.0";

  # root is tmpfs. the store is the verity partition, everything personal is on persist
  fileSystems."/" = {
    fsType = "tmpfs";
    options = [
      "mode=0755"
      "size=25%"
    ];
  };

  image.repart = {
    name = "eclipse";
    verityStore = {
      enable = true;
      # +3 is the boot counter: three tries before systemd-boot falls back to the other slot
      ukiPath = "/EFI/Linux/${id}_${version}+3.efi";
    };
    partitions = {
      "00-esp" = {
        # the firmware starts systemd-boot from the removable media path, nothing is written to its
        # variables. no menu unless a key is held, and no editing the command line
        contents = {
          "/EFI/BOOT/BOOTX64.EFI".source = systemdBoot;
          "/EFI/systemd/systemd-bootx64.efi".source = systemdBoot;
          "/loader/loader.conf".source = pkgs.writeText "loader.conf" ''
            timeout 0
            editor no
          '';
        };
        repartConfig = {
          Type = "esp";
          Format = "vfat";
          Label = "esp";
          SizeMinBytes = "1G";
          SizeMaxBytes = "1G";
        };
      };
      # a fixed 1G, enough for the hash tree of a full 8G store, so a later version fits here too
      "10-store-verity" = {
        repartConfig = {
          Type = "usr-verity";
          Label = "store-verity_${version}";
          Minimize = "off";
          SizeMinBytes = "1G";
          SizeMaxBytes = "1G";
        };
      };
      # as small as what is in it, and the last partition, so the flash step can grow it to its 8G slot
      "20-store" = {
        repartConfig = {
          Type = "usr";
          Label = "store_${version}";
          Minimize = "best";
          SizeMaxBytes = "8G";
        };
      };
    };
  };

  # no nix-daemon on the device yet: the store is read-only and root is tmpfs, the db wouldn't survive a boot.
  # a writable overlay store on persist comes later
  nix.enable = false;
}
