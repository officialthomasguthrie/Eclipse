# airlock: sandboxing. bwrap for rift run --sandbox, which airlock runs unprivileged in a user
# namespace, each sandbox in a scope of the user manager named for its app. airlock serve answers on
# the system bus as dev.rift.Airlock and keeps the network switch for those apps in its own
# nftables table. flatpak with its portals for gui apps has a switch of its own. no host disk is
# visible to any sandbox.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.rift.airlock;
  busName = "dev.rift.Airlock";
  # anyone may list the switch, and a sandbox asks as it starts, from its own scope. only the owner,
  # who is in wheel, turns an app's network off or on. only root owns the name
  policy = pkgs.writeTextFile {
    name = "airlock-dbus-policy";
    destination = "/share/dbus-1/system.d/${busName}.conf";
    text = ''
      <!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
       "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
      <busconfig>
        <policy user="root">
          <allow own="${busName}"/>
          <allow send_destination="${busName}" send_interface="${busName}"/>
        </policy>
        <policy context="default">
          <allow send_destination="${busName}" send_interface="${busName}" send_member="List"/>
          <allow send_destination="${busName}" send_interface="${busName}" send_member="Starting"/>
          <allow send_destination="${busName}" send_interface="org.freedesktop.DBus.Introspectable"/>
          <allow send_destination="${busName}" send_interface="org.freedesktop.DBus.Peer"/>
        </policy>
        <policy group="wheel">
          <allow send_destination="${busName}" send_interface="${busName}" send_member="SetNetwork"/>
        </policy>
      </busconfig>
    '';
  };
in
{
  options.rift.airlock = {
    enable = lib.mkEnableOption "Airlock, app sandboxing";
    flatpak.enable = lib.mkEnableOption "Flatpak with portals for graphical apps";
    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.workspace;
      description = "The build that provides the airlock binary.";
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        environment.systemPackages = [
          pkgs.bubblewrap
          pkgs.nftables
        ];
        # host disks are never auto-mounted. orbit mounts them read-only on request
        services.udisks2.enable = lib.mkForce false;
        services.dbus.packages = [ policy ];

        systemd.services.airlock = {
          description = "Airlock";
          wantedBy = [ "multi-user.target" ];
          requires = [ "dbus.service" ];
          # nixos's own rules come first. they leave other tables alone
          after = [
            "dbus.service"
            "nftables.service"
          ];
          path = [ pkgs.nftables ];
          serviceConfig = {
            Type = "dbus";
            BusName = busName;
            ExecStart = "${cfg.package}/bin/airlock serve --state /var/lib/rift/airlock";
            Restart = "on-failure";
            # the apps that are off. the table stays when the service stops, so they stay off
            StateDirectory = "rift/airlock";
            # root with only the right to change the firewall. it reads the cgroup of the process
            # that asks, which anyone may
            CapabilityBoundingSet = [ "CAP_NET_ADMIN" ];
            ProtectSystem = "strict";
            ProtectHome = true;
            PrivateTmp = true;
            NoNewPrivileges = true;
          };
        };
      }
      (lib.mkIf cfg.flatpak.enable {
        services.flatpak.enable = true;
        # xdg-desktop-portal answers documents, the network monitor, proxies, trash and a few more
        # by itself, over the session bus. gtk's backend is the rest: the file chooser, the app
        # chooser, printing and the appearance settings, drawn in horizon's session
        xdg.portal = {
          enable = true;
          extraPortals = [ pkgs.xdg-desktop-portal-gtk ];
          config.common.default = [ "gtk" ];
        };
      })
    ]
  );
}
