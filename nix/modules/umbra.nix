# umbra: the compositor. the niri fork lands in crates/umbra later, upstream niri stands in until then.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.eclipse.umbra;
in
{
  options.eclipse.umbra.enable = lib.mkEnableOption "Umbra, the Eclipse compositor";

  config = lib.mkIf cfg.enable {
    programs.niri.enable = true; # stand-in
    programs.niri.package = pkgs.niri;
    xdg.portal.enable = true;
    environment.systemPackages = with pkgs; [
      ghostty
      firefox
      zed-editor
      wl-clipboard
    ];
  };
}
