# penumbra: sandboxing. flatpak + portals for gui apps, bwrap for cli tools, per-app network switch.
# no host disk is visible to any sandbox.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.eclipse.penumbra;
in
{
  options.eclipse.penumbra.enable = lib.mkEnableOption "Penumbra, app sandboxing";

  config = lib.mkIf cfg.enable {
    services.flatpak.enable = true;
    xdg.portal.enable = true;
    environment.systemPackages = with pkgs; [
      bubblewrap
    ];
    # host disks are never auto-mounted. syzygy mounts them read-only on request
    services.udisks2.enable = lib.mkForce false;
  };
}
