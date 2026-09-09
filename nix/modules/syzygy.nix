# syzygy: host adaptation. runs before the session, fingerprints the host, loads or creates its profile,
# publishes it on d-bus. for now it only writes the profile under the hosts directory.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.eclipse.syzygy;
in
{
  options.eclipse.syzygy = {
    enable = lib.mkEnableOption "Syzygy, host adaptation";
    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.workspace;
      description = "The build that provides the syzygy binary.";
    };
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
      unitConfig.RequiresMountsFor = cfg.hostsDir;
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = "${cfg.package}/bin/syzygy --hosts-dir ${cfg.hostsDir}";
        # reads /sys, writes only the hosts directory
        ProtectSystem = "strict";
        ReadWritePaths = [ cfg.hostsDir ];
        ProtectHome = true;
        PrivateTmp = true;
        PrivateNetwork = true;
        NoNewPrivileges = true;
      };
    };
  };
}
