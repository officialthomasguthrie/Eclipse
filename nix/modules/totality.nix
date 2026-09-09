# boot splash. a black disc crosses a light one while the initrd runs, totality is when the session
# takes the display. the luks prompt sits under the disc. tools/boot-test.py checks the screen.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.eclipse.totality;
  theme = pkgs.stdenvNoCC.mkDerivation {
    pname = "plymouth-theme-totality";
    version = "0.1.0";
    src = ../totality/plymouth;
    installPhase = ''
      mkdir -p $out/share/plymouth/themes/totality
      cp -r $src/. $out/share/plymouth/themes/totality/
      # the theme file names its own location. the initrd copy only rewrites store paths
      substituteInPlace $out/share/plymouth/themes/totality/totality.plymouth \
        --replace-fail /usr/share/plymouth/themes/totality $out/share/plymouth/themes/totality
    '';
  };
in
{
  options.eclipse.totality.enable = lib.mkEnableOption "the Totality boot splash";

  config = lib.mkIf cfg.enable {
    boot.plymouth = {
      enable = true;
      theme = "totality";
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
