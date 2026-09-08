# persist: luks2 around btrfs, one subvolume per kind of personal data.
# the flasher creates it (tools/flash.sh for now), not the image build, since it's whatever is left of the stick.
{
  config,
  lib,
  ...
}:
let
  mountOptions = [
    "compress=zstd:3"
    "noatime"
    "ssd"
    "discard=async"
    "commit=15"
  ];
  subvol = name: {
    device = "/dev/mapper/persist";
    fsType = "btrfs";
    options = [ "subvol=${name}" ] ++ mountOptions;
    neededForBoot = true;
  };
in
{
  boot.initrd.luks.devices.persist = {
    device = "/dev/disk/by-partlabel/persist";
    allowDiscards = true;
    # fido2 and tpm2 get enrolled with systemd-cryptenroll later
  };

  fileSystems = {
    "/persist" = {
      device = "/dev/mapper/persist";
      fsType = "btrfs";
      options = [ "subvolid=5" ] ++ mountOptions;
      neededForBoot = true;
    };
    "/home" = subvol "@home";
    "/var" = subvol "@var";
    "/var/lib/flatpak" = subvol "@flatpak";
    "/var/lib/eclipse/models" = subvol "@models";
    "/var/lib/eclipse/hosts" = subvol "@hosts";
  };

  # compressed swap in ram, never on the stick
  zramSwap = {
    enable = true;
    algorithm = "zstd";
    memoryPercent = 50;
  };
  swapDevices = [ ];

  # keep the machine id across boots even though root is tmpfs
  environment.etc."machine-id".source = "/var/lib/eclipse/machine-id";
  systemd.tmpfiles.rules = [
    "d /var/lib/eclipse 0755 root root -"
    "d /var/lib/eclipse/aura 0750 root root -"
  ];
}
