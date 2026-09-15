# orbit: host adaptation. runs before the session, works out what the machine is, writes its
# profile under the hosts directory, then answers on the system bus as dev.rift.Orbit.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.rift.orbit;
  busName = "dev.rift.Orbit";
  # the interface is read only, so anyone on the machine may read it. only root owns the name.
  policy = pkgs.writeTextFile {
    name = "orbit-dbus-policy";
    destination = "/share/dbus-1/system.d/${busName}.conf";
    text = ''
      <!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
       "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
      <busconfig>
        <policy user="root">
          <allow own="${busName}"/>
        </policy>
        <policy context="default">
          <allow send_destination="${busName}" send_interface="${busName}"/>
          <allow send_destination="${busName}" send_interface="org.freedesktop.DBus.Properties"/>
          <allow send_destination="${busName}" send_interface="org.freedesktop.DBus.Introspectable"/>
          <allow send_destination="${busName}" send_interface="org.freedesktop.DBus.Peer"/>
        </policy>
      </busconfig>
    '';
  };
in
{
  options.rift.orbit = {
    enable = lib.mkEnableOption "Orbit, host adaptation";
    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.workspace;
      description = "The build that provides the orbit binary.";
    };
    hostsDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/rift/hosts";
      description = "Per-host profiles (the @hosts subvolume on persist).";
    };
  };

  config = lib.mkIf cfg.enable {
    services.dbus.packages = [ policy ];

    systemd.services.orbit = {
      description = "Orbit host adaptation";
      wantedBy = [ "multi-user.target" ];
      before = [
        "display-manager.service"
        "getty@tty1.service"
      ];
      requires = [ "dbus.service" ];
      after = [
        "local-fs.target"
        "dbus.service"
      ];
      unitConfig.RequiresMountsFor = cfg.hostsDir;
      serviceConfig = {
        # the profile is written before the name is taken, so anything that waits for the name
        # on the bus already has the answers when it arrives
        Type = "dbus";
        BusName = busName;
        ExecStart = "${cfg.package}/bin/orbit --hosts-dir ${cfg.hostsDir} --serve";
        Restart = "on-failure";
        # reads /sys, writes only the hosts directory
        ProtectSystem = "strict";
        ReadWritePaths = [ cfg.hostsDir ];
        ProtectHome = true;
        PrivateTmp = true;
        # the bus is a unix socket, so it is still there without a network
        PrivateNetwork = true;
        NoNewPrivileges = true;
      };
    };
  };
}
