# boot splash. a black disc crosses a light one while the initrd runs, liftoff is when the session
# takes the display. the luks prompt sits under the disc. tools/boot-test.py checks the screen.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.rift.liftoff;
  theme = pkgs.stdenvNoCC.mkDerivation {
    pname = "plymouth-theme-liftoff";
    version = "0.1.0";
    src = ../liftoff/plymouth;
    installPhase = ''
      mkdir -p $out/share/plymouth/themes/liftoff
      cp -r $src/. $out/share/plymouth/themes/liftoff/
      # the theme file names its own location. the initrd copy only rewrites store paths
      substituteInPlace $out/share/plymouth/themes/liftoff/liftoff.plymouth \
        --replace-fail /usr/share/plymouth/themes/liftoff $out/share/plymouth/themes/liftoff
    '';
  };
in
{
  options.rift.liftoff.enable = lib.mkEnableOption "the Liftoff boot splash";

  config = lib.mkIf cfg.enable {
    boot.plymouth = {
      enable = true;
      theme = "liftoff";
      themePackages = [ theme ];
      # the initrd has no gpu drivers, only the firmware framebuffer through simpledrm. plymouth
      # waits 8 s for a real drm device before it touches simpledrm unless told otherwise, and the
      # luks prompt comes before that.
      extraConfig = "UseSimpledrm=1";
    };
    boot.kernelParams = [
      "quiet"
      "splash"
      "loglevel=3"
      "rd.systemd.show_status=false"
      "udev.log_level=3"
    ];
    boot.consoleLogLevel = 3;
    boot.initrd.verbose = false;
  };
}
