# aura: local ai. aurad supervises llama-server (vulkan + cpu), whisper and piper, and exposes
# dev.eclipse.Aura on d-bus plus an openai-style api on localhost. only the backend is wired for now.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.eclipse.aura;
in
{
  options.eclipse.aura = {
    enable = lib.mkEnableOption "Aura, the local AI service";

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
      type = lib.types.str;
      default = "Qwen3-4B-Q4_K_M.gguf";
      description = "Default chat model file. Syzygy overrides this per host tier.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 11434;
      description = "Localhost port for the OpenAI-compatible API.";
    };

    contextSize = lib.mkOption {
      type = lib.types.int;
      default = 8192;
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];

    systemd.services.aura-inference = {
      description = "Aura inference backend (llama-server)";
      wantedBy = [ "multi-user.target" ];
      # the condition is checked when the unit starts, so it has to run after the mount
      unitConfig = {
        RequiresMountsFor = [ cfg.modelsDir ];
        ConditionPathExists = "${cfg.modelsDir}/${cfg.model}";
      };
      serviceConfig = {
        ExecStart = lib.concatStringsSep " " [
          "${cfg.package}/bin/llama-server"
          "--host 127.0.0.1"
          "--port ${toString cfg.port}"
          "--model ${cfg.modelsDir}/${cfg.model}"
          "--ctx-size ${toString cfg.contextSize}"
        ];
        Restart = "on-failure";
        DynamicUser = true;
        SupplementaryGroups = [
          "render"
          "video"
        ];
        DeviceAllow = [ "char-drm rw" ];
        ReadOnlyPaths = [ cfg.modelsDir ];
        # aura never talks to the network, localhost only
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
