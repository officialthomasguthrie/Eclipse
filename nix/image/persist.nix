# persist: luks2 around btrfs, one subvolume per kind of personal data.
# it is whatever is left of the stick, so the image build never makes it. eclipse-flash makes it on
# linux; a drive written on macos or windows, or with --first-boot, makes it at its first boot.
{
  config,
  lib,
  pkgs,
  self,
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
  initrdSystemd = config.boot.initrd.systemd.package;
  firstBoot = "${self.packages.${pkgs.stdenv.hostPlatform.system}.workspace}/bin/vault-first-boot";
in
{
  boot.initrd.luks.devices.persist = {
    device = "/dev/disk/by-partlabel/persist";
    allowDiscards = true;
    # a new drive has no persist partition until its passphrase is chosen, which takes longer than
    # the 90 s systemd waits for a device. a device job cannot be ordered after a service
    crypttabExtraOpts = [ "x-systemd.device-timeout=infinity" ];
    # fido2 and tpm2 get enrolled with systemd-cryptenroll later
  };

  # vault-first-boot runs before persist is opened. on a drive without persist it asks for a
  # passphrase on the splash, makes persist in the free space and opens it, so systemd-cryptsetup
  # finds it open and asks nothing. it also formats an exchange partition that has no file system.
  # when it fails the boot stops with its message instead of waiting for persist for ever
  boot.initrd.systemd = {
    storePaths = [
      firstBoot
      "${initrdSystemd}/bin/systemd-ask-password"
    ];
    initrdBin = [ pkgs.cryptsetup ];
    extraBin = {
      blkid = "${pkgs.util-linux}/bin/blkid";
      blockdev = "${pkgs.util-linux}/bin/blockdev";
      lsblk = "${pkgs.util-linux}/bin/lsblk";
      partx = "${pkgs.util-linux}/bin/partx";
      sfdisk = "${pkgs.util-linux}/bin/sfdisk";
      "mkfs.exfat" = "${pkgs.exfatprogs}/bin/mkfs.exfat";
    };
    services.vault-first-boot = {
      description = "Make persist on a new drive";
      wantedBy = [ "initrd.target" ];
      wants = [ "cryptsetup-pre.target" ];
      after = [
        "systemd-udev-trigger.service"
        "plymouth-start.service"
        "systemd-ask-password-plymouth.path"
        "systemd-ask-password-console.path"
      ];
      before = [
        "cryptsetup-pre.target"
        "systemd-cryptsetup@persist.service"
      ];
      unitConfig = {
        DefaultDependencies = false;
        OnFailure = "emergency.target";
      };
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = firstBoot;
        TimeoutStartSec = "infinity";
      };
    };
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
