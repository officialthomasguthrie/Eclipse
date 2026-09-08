# syzygy: host adaptation. runs before the session, fingerprints the host, loads or creates its profile,
# publishes it on d-bus. runs the stub binary for now.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.eclipse.syzygy;
in
{
  options.eclipse.syzygy = {
    enable = lib.mkEnableOption "Syzygy, host adaptation";
    hostsDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/eclipse/hosts";
      description = "Per-host profiles (the @hosts subvolume on persist).";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.syzygy = {
      description = "Syzygy host adaptation";
      wantedBy = [ "multi-user.target" ];
      before = [
        "display-manager.service"
        "getty@tty1.service"
      ];
      after = [ "local-fs.target" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = "/run/current-system/sw/bin/syzygy --hosts-dir ${cfg.hostsDir}";
      };
    };
  };
}
