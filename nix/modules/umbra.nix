# umbra: the compositor. the fork is in crates/umbra and the flake builds it as the umbra package,
# but the session start still runs on upstream niri from nixpkgs until the module moves over.
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
