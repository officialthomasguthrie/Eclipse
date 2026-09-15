# what every eclipse image has
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  system = pkgs.stdenv.hostPlatform.system;
  eclipseWorkspace = self.packages.${system}.workspace;
in
{
  networking.hostName = "eclipse";
  networking.networkmanager = {
    enable = true;
    wifi.backend = "iwd";
  };
  networking.nftables.enable = true;
  networking.firewall.enable = true;

  time.timeZone = lib.mkDefault "UTC"; # syzygy and first boot set the real one
  i18n.defaultLocale = "en_US.UTF-8";

  services.pipewire = {
    enable = true;
    alsa.enable = true;
    pulse.enable = true;
  };
  security.rtkit.enable = true;
  hardware.bluetooth.enable = true;

  # its greeting is in identity.nix
  programs.fish.enable = true;
  documentation.man.enable = true; # aura indexes man pages offline

  # dev account until first boot setup replaces it with the real owner
  users.mutableUsers = false;
  users.users.eclipse = {
    isNormalUser = true;
    description = "Eclipse owner";
    extraGroups = [
      "wheel"
      "networkmanager"
      "video"
      "input"
      "render"
    ];
    shell = pkgs.fish;
    initialPassword = "eclipse";
  };
  security.sudo.wheelNeedsPassword = false;

  environment.systemPackages = with pkgs; [
    eclipseWorkspace # aurad, syzygy, corona, eclipse, ...
    helix
    zellij
    nushell
    git
    curl
    ripgrep
    fd
    btop
    pciutils
    usbutils
    dmidecode
    btrfs-progs
    cryptsetup
    gptfdisk
  ];

  system.stateVersion = "26.05";
}
