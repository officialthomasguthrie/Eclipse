# aura: local ai. aurad picks a chat model from the manifest for the tier syzygy reports, runs
# llama-server (vulkan + cpu) as its child on a unix socket only aura's user can open, serves the
# local api on 127.0.0.1 in front of it and answers on the system bus as dev.eclipse.Aura.
# whisper and piper come later.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.eclipse.aura;
  busName = "dev.eclipse.Aura";
  # anyone on the machine may ask and read the properties. only aura's own user owns the name
  policy = pkgs.writeTextFile {
    name = "aura-dbus-policy";
    destination = "/share/dbus-1/system.d/${busName}.conf";
    text = ''
      <!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
       "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
      <busconfig>
        <policy user="aura">
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
  options.eclipse.aura = {
    enable = lib.mkEnableOption "Aura, the local AI service";

    daemon = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.workspace;
      description = "The build that provides aurad.";
    };

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.llama-cpp.override { vulkanSupport = true; };
      description = "llama.cpp build used for inference. Vulkan so it works on any GPU vendor.";
    };

    modelsDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/eclipse/models";
      description = "Where the GGUF weights live (the @models subvolume on persist).";
    };

    model = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "A chat model id or file from the manifest to run instead of the one the tier picks.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 11434;
      description = "Localhost port for the OpenAI-compatible API. aurad serves it and refuses requests from web pages.";
    };

    contextSize = lib.mkOption {
      type = lib.types.int;
      default = 8192;
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    # the one list of models. aurad reads it here
    environment.etc."eclipse/models.toml".source = ../../models/manifest.toml;
    services.dbus.packages = [ policy ];

    users.users.aura = {
      isSystemUser = true;
      group = "aura";
      description = "Aura";
    };
    users.groups.aura = { };

    systemd.services.aura = {
      description = "Aura, the local AI service";
      wantedBy = [ "multi-user.target" ];
      requires = [ "dbus.service" ];
      # syzygy counts as started once its name is on the bus, and by then it knows the tier
      after = [
        "dbus.service"
        "syzygy.service"
      ];
      unitConfig.RequiresMountsFor = [ cfg.modelsDir ];
      serviceConfig = {
        # aurad takes the name once it has the tier. the model loads after that, the State
        # property says when it is ready
        Type = "dbus";
        BusName = busName;
        ExecStart = lib.concatStringsSep " " (
          [
            "${cfg.daemon}/bin/aurad"
            "--manifest /etc/eclipse/models.toml"
            "--models-dir ${cfg.modelsDir}"
            "--llama-server ${cfg.package}/bin/llama-server"
            "--port ${toString cfg.port}"
            "--socket /run/aura/llama.sock"
            "--ctx-size ${toString cfg.contextSize}"
          ]
          ++ lib.optional (cfg.model != null) "--model ${cfg.model}"
        );
        Restart = "on-failure";
        # llama-server's socket. nobody else may open it, the local api is the way in
        RuntimeDirectory = "aura";
        RuntimeDirectoryMode = "0700";
        User = "aura";
        Group = "aura";
        # llama-server is aurad's child, everything below holds for it too
        SupplementaryGroups = [
          "render"
          "video"
        ];
        DeviceAllow = [ "char-drm rw" ];
        ReadOnlyPaths = [ cfg.modelsDir ];
        # aura never talks to the network, localhost only. the bus is a unix socket
        IPAddressDeny = "any";
        IPAddressAllow = [ "localhost" ];
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        NoNewPrivileges = true;
      };
    };
  };
}
