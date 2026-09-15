# corona: the shell. one field in a panel along the top of the screen, four interpreters behind
# it (launcher, os commands, nushell, aura). umbra starts it with the session.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.rift.corona;
in
{
  options.rift.corona.enable = lib.mkEnableOption "Corona, the Rift shell";

  config = lib.mkIf cfg.enable {
    # the corona binary comes with the workspace package in profiles/base.nix. it is a layer-shell
    # client, so the compositor spawns it once the display is up. a user unit would need a
    # graphical session target, and the session does not run under the user's systemd yet.
    # umbra drops what its children print, systemd-cat puts corona's log in the journal
    rift.umbra.startup = [
      [
        "${config.systemd.package}/bin/systemd-cat"
        "-t"
        "corona"
        "corona"
      ]
    ];
    # what the os commands run is already there: nmcli with networkmanager, wpctl with pipewire,
    # brightnessctl with umbra, systemctl always. nushell is the third interpreter: corona runs
    # this binary with an argument vector, and it is the interactive nushell as well
    environment.systemPackages = [ pkgs.nushell ];
  };
}
