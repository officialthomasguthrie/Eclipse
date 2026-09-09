# corona: the shell. one field in a panel along the top of the screen, four interpreters behind
# it (launcher, os commands, nushell, aura). umbra starts it with the session.
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
    # the corona binary comes with the workspace package in profiles/base.nix. it is a layer-shell
    # client, so the compositor spawns it once the display is up. a user unit would need a
    # graphical session target, and the session does not run under the user's systemd yet.
    # umbra drops what its children print, systemd-cat puts corona's log in the journal
    eclipse.umbra.startup = [
      [
        "${config.systemd.package}/bin/systemd-cat"
        "-t"
        "corona"
        "corona"
      ]
    ];
    # what the os commands run is already there: nmcli with networkmanager, wpctl with pipewire,
    # brightnessctl with umbra, systemctl always
    environment.systemPackages = [ pkgs.nushell ];
  };
}
