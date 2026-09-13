# penumbra: sandboxing. bwrap for eclipse run --sandbox, which penumbra runs unprivileged in a user
# namespace. flatpak with portals for gui apps has its own switch until it is tested, and the per-app
# network switch comes later. no host disk is visible to any sandbox.
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
  options.eclipse.penumbra = {
    enable = lib.mkEnableOption "Penumbra, app sandboxing";
    flatpak.enable = lib.mkEnableOption "Flatpak with portals for graphical apps";
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        environment.systemPackages = [ pkgs.bubblewrap ];
        # host disks are never auto-mounted. syzygy mounts them read-only on request
        services.udisks2.enable = lib.mkForce false;
      }
      (lib.mkIf cfg.flatpak.enable {
        services.flatpak.enable = true;
        xdg.portal.enable = true;
      })
    ]
  );
}
