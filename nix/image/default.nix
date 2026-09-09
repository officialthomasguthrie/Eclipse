# totality: the read-only, verity-checked system image
#
# for now: one slot, the uki is the default efi entry, no bootloader
# later: a/b slots, systemd-boot, sysupdate. see ab-sysupdate.nix
{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:
{
  imports = [
    "${modulesPath}/image/repart.nix"
    "${modulesPath}/image/repart-verity-store.nix"
    ./persist.nix
  ];

  boot.loader.grub.enable = false;
  boot.initrd.systemd.enable = true;

  # names the uki and the image file: eclipse_<version>
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
      # no boot manager yet: the uki sits at the removable media path, the one place firmware looks on its own
      ukiPath = "/EFI/BOOT/BOOTX64.EFI";
    };
    partitions = {
      "00-esp" = {
        repartConfig = {
          Type = "esp";
          Format = "vfat";
          Label = "esp";
          SizeMinBytes = "1G";
          SizeMaxBytes = "1G";
        };
      };
      "10-store-verity" = {
        repartConfig = {
          Type = "usr-verity";
          Minimize = "best";
        };
      };
      "20-store" = {
        repartConfig = {
          Type = "usr";
          Minimize = "best";
        };
      };
    };
  };

  # no nix-daemon on the device yet: the store is read-only and root is tmpfs, the db wouldn't survive a boot.
  # a writable overlay store on persist comes later
  nix.enable = false;
}
