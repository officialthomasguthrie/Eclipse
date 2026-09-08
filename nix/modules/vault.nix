# vault: snapshots, rustic backups, drive cloning
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.eclipse.vault;
in
{
  options.eclipse.vault.enable = lib.mkEnableOption "Vault, snapshots and backups";

  config = lib.mkIf cfg.enable {
    environment.systemPackages = with pkgs; [
      rustic
      btrfs-progs
    ];
    # usb flash lies about its health, scrub monthly
    services.btrfs.autoScrub = {
      enable = true;
      fileSystems = [ "/persist" ];
      interval = "monthly";
    };
    # timeline (hourly/daily/weekly snapshots + scrubber ui) comes later
  };
}
