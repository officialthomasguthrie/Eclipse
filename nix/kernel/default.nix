# kernel: as much hardware as possible, root fs on a usb stick
{
  config,
  lib,
  pkgs,
  ...
}:
{
  boot.kernelPackages = lib.mkDefault pkgs.linuxPackages_latest;

  boot.kernelParams = [
    # iommu on so a hostile thunderbolt device can't read ram
    "iommu=pt"
    "intel_iommu=on"
    "amd_iommu=on"
    # zram does the swapping
    "zswap.enabled=0"
    # never autosuspend the drive we're running from
    "usbcore.autosuspend=-1"
  ];

  boot.kernelModules = [
    "kvm-intel"
    "kvm-amd"
  ];

  # enough to find and open the stick from the initrd
  boot.initrd.availableKernelModules = [
    "xhci_pci"
    "ehci_pci"
    "uhci_hcd"
    "usb_storage"
    "uas"
    "sd_mod"
    "nvme"
    "thunderbolt"
    "dm_mod"
    "dm_verity"
    "dm_crypt"
    "erofs"
    "btrfs"
    "vfat"
    "exfat"
  ];
  boot.supportedFilesystems = [
    "btrfs"
    "erofs"
    "vfat"
    "exfat"
  ];

  # wifi is the usual failure, ship every blob
  hardware.enableAllFirmware = true;
  hardware.cpu.intel.updateMicrocode = true;
  hardware.cpu.amd.updateMicrocode = true;
  nixpkgs.config.allowUnfree = true;

  # mesa for intel and amd, nvk for nvidia. proprietary nvidia is an opt-in download later
  hardware.graphics.enable = true;

  # zram tuning
  boot.kernel.sysctl = {
    "vm.swappiness" = 180;
    "vm.watermark_boost_factor" = 0;
    "vm.watermark_scale_factor" = 125;
    "vm.page-cluster" = 0;
  };

  # never touch the host's firmware
  services.fwupd.enable = false;
}
