# vault: timeline snapshots of home, rustic backups, drive cloning. vault serve answers on the
# system bus as dev.eclipse.Vault and a timer takes a snapshot every hour. backups and cloning come
# later
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.eclipse.vault;
  busName = "dev.eclipse.Vault";
  # anyone on the machine may list, take and restore. a restore runs as the account that asked,
  # and every take runs the retention rules. only root owns the name
  policy = pkgs.writeTextFile {
    name = "vault-dbus-policy";
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
          <allow send_destination="${busName}" send_interface="org.freedesktop.DBus.Introspectable"/>
          <allow send_destination="${busName}" send_interface="org.freedesktop.DBus.Peer"/>
        </policy>
      </busconfig>
    '';
  };
  # the service and the timer use the same places and the same rules
  timeline = lib.concatStringsSep " " [
    "--subvolume /persist/@home"
    "--snapshots /persist/@snapshots/home"
    "--hourly ${toString cfg.timeline.hourly}"
    "--daily ${toString cfg.timeline.daily}"
    "--weekly ${toString cfg.timeline.weekly}"
  ];
  keepOption =
    default: what:
    lib.mkOption {
      type = lib.types.ints.unsigned;
      inherit default;
      description = "How many of the latest ${what} keep their first snapshot.";
    };
in
{
  options.eclipse.vault = {
    enable = lib.mkEnableOption "Vault, snapshots and backups";
    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.workspace;
      description = "The build that provides the vault binary.";
    };
    timeline = {
      hourly = keepOption 24 "hours";
      daily = keepOption 7 "days";
      weekly = keepOption 8 "weeks";
    };
  };

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
    services.dbus.packages = [ policy ];

    systemd.services.vault = {
      description = "Vault";
      wantedBy = [ "multi-user.target" ];
      requires = [ "dbus.service" ];
      after = [
        "local-fs.target"
        "dbus.service"
      ];
      unitConfig.RequiresMountsFor = [
        "/persist"
        "/home"
      ];
      path = [ pkgs.btrfs-progs ];
      serviceConfig = {
        Type = "dbus";
        BusName = busName;
        ExecStart = "${cfg.package}/bin/vault serve --home /home ${timeline}";
        Restart = "on-failure";
        # root, because taking and deleting a snapshot needs it. a restore reads and writes in a
        # child that runs as the account that asked
        ProtectSystem = "strict";
        ReadWritePaths = [
          "/persist"
          "/home"
        ];
        PrivateTmp = true;
        # the bus is a unix socket, so it is still there without a network
        PrivateNetwork = true;
        NoNewPrivileges = true;
      };
    };

    # the hourly snapshot. systemd catches up once at boot when the drive was off at the hour
    systemd.services.vault-timeline = {
      description = "Vault, the hourly snapshot of home";
      unitConfig.RequiresMountsFor = [ "/persist" ];
      path = [ pkgs.btrfs-progs ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${cfg.package}/bin/vault take ${timeline}";
        ProtectSystem = "strict";
        ReadWritePaths = [ "/persist" ];
        ProtectHome = true;
        PrivateTmp = true;
        PrivateNetwork = true;
        NoNewPrivileges = true;
      };
    };
    systemd.timers.vault-timeline = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnCalendar = "hourly";
        Persistent = true;
      };
    };
  };
}
