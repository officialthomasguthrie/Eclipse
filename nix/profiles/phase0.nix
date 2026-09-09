# phase 0 plus the first of phase 1: console, shell, splash, aura's backend if a model is there,
# umbra on tty1 and corona's panel on it. the serial console keeps its autologin shell, the boot test talks to it.
{ config, ... }:
{
  boot.kernelParams = [
    "console=ttyS0,115200"
    "console=tty1"
    # plymouth drops to its text mode on every console as soon as it sees a serial one
    "plymouth.ignore-serial-consoles"
  ];
  services.getty.autologinUser = "eclipse";

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
  eclipse.aura.enable = true;
  # the smallest chat model in the manifest. the boot test copies it into @models before boot
  eclipse.aura.model = "Qwen3-0.6B-Q8_0.gguf";
  eclipse.umbra.enable = true;
  eclipse.corona.enable = true;
  eclipse.penumbra.enable = false;
  eclipse.vault.enable = true;

  environment.etc."eclipse/phase".text = "0\n";
}
