# corona: the shell. one field, four interpreters (launcher, os commands, nushell, aura).
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.eclipse.corona;
in
{
  options.eclipse.corona.enable = lib.mkEnableOption "Corona, the Eclipse shell";

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ pkgs.nushell ];
    # the corona binary comes with the workspace package in profiles/base.nix.
    # a user service will start it as a layer-shell client of umbra.
  };
}
