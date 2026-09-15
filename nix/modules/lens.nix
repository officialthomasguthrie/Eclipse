# lens: the shell. one field in a panel along the top of the screen, four interpreters behind
# it (launcher, os commands, nushell, quasar). horizon starts it with the session.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.rift.lens;
in
{
  options.rift.lens.enable = lib.mkEnableOption "Lens, the Rift shell";

  config = lib.mkIf cfg.enable {
    # the lens binary comes with the workspace package in profiles/base.nix. it is a layer-shell
    # client, so the compositor spawns it once the display is up. a user unit would need a
    # graphical session target, and the session does not run under the user's systemd yet.
    # horizon drops what its children print, systemd-cat puts lens's log in the journal
    rift.horizon.startup = [
      [
        "${config.systemd.package}/bin/systemd-cat"
        "-t"
        "lens"
        "lens"
      ]
    ];
    # what the os commands run is already there: nmcli with networkmanager, wpctl with pipewire,
    # brightnessctl with horizon, systemctl always. nushell is the third interpreter: lens runs
    # this binary with an argument vector, and it is the interactive nushell as well
    environment.systemPackages = [ pkgs.nushell ];
  };
}
