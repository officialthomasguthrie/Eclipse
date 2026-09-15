# phase 0 plus the first of phase 1: console, shell, splash, aura's backend if a model is there,
# umbra on tty1 and corona's panel on it. the serial console keeps its autologin shell, the boot test talks to it.
{ config, pkgs, ... }:
{
  boot.kernelParams = [
    "console=ttyS0,115200"
    "console=tty1"
    # plymouth drops to its text mode on every console as soon as it sees a serial one
    "plymouth.ignore-serial-consoles"
  ];
  # the serial console logs the owner in by itself. the text consoles ask for a login, so a locked
  # session stays locked when someone switches to one
  systemd.services."serial-getty@ttyS0" = {
    overrideStrategy = "asDropin";
    serviceConfig.ExecStart = [
      ""
      "${pkgs.util-linux}/bin/agetty --login-program ${config.services.getty.loginProgram} --issue-file /etc/issue:/etc/issue.d:/run/issue:/run/issue.d --autologin eclipse %I --keep-baud $TERM"
    ];
  };

  # the luks prompt also on the serial console. systemd's console agent stays out of plymouth's way
  # by default (both its path and service units check for plymouth), so it runs on ttyS0 only
  # and leaves tty1 to the splash.
  boot.initrd.systemd.paths.systemd-ask-password-console = {
    wantedBy = [ "sysinit.target" ];
    overrideStrategy = "asDropin";
    unitConfig.ConditionPathExists = "";
  };
  boot.initrd.systemd.services.systemd-ask-password-console = {
    overrideStrategy = "asDropin";
    unitConfig.ConditionPathExists = "";
    serviceConfig.ExecStart = [
      ""
      "${config.boot.initrd.systemd.package}/bin/systemd-tty-ask-password-agent --watch --console=/dev/ttyS0"
    ];
  };

  # the journal is mirrored to the serial console: the boot test reads it, nothing else can yet
  boot.initrd.systemd.contents."/etc/systemd/journald.conf".text = ''
    [Journal]
    ForwardToConsole=yes
    TTYPath=/dev/ttyS0
    MaxLevelConsole=info
  '';
  services.journald.settings.Journal = {
    ForwardToConsole = true;
    TTYPath = "/dev/ttyS0";
    MaxLevelConsole = "info";
  };

  eclipse.totality.enable = true;
  eclipse.syzygy.enable = true;
  # aurad picks the model for the tier. the boot test copies only the test model into @models,
  # so that is what runs there
  eclipse.aura.enable = true;
  eclipse.umbra.enable = true;
  eclipse.corona.enable = true;
  # bwrap for eclipse run --sandbox, and flatpak with its portals
  eclipse.penumbra.enable = true;
  eclipse.penumbra.flatpak.enable = true;
  eclipse.vault.enable = true;

  environment.etc."eclipse/phase".text = "0\n";
}
