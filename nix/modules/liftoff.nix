# the boot splash. by default a text boot: the logo in characters and the system's name at the top,
# and under them systemd's status lines as they reach the console, drawn by liftoff-splash, a plymouth
# plugin. the graphical style is the script theme, the mark in the middle of the same near black. both
# styles are in the initrd, and plymouth.splash=liftoff-text or liftoff-graphical on the kernel command
# line picks one for a boot. tools/boot-test.py checks both screens.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.rift.liftoff;
  splash = self.packages.${pkgs.stdenv.hostPlatform.system}.liftoff-splash;
  fonts = "${pkgs.dejavu_fonts}/share/fonts/truetype";
  title = "${config.system.nixos.distroName} ${config.system.image.version}";
  themes = pkgs.stdenvNoCC.mkDerivation {
    pname = "plymouth-themes-liftoff";
    inherit (config.system.image) version;
    src = ../liftoff/plymouth;
    installPhase = ''
      # each theme file names its own location. the initrd copy only rewrites store paths
      graphical=$out/share/plymouth/themes/liftoff-graphical
      mkdir -p $graphical
      cp $src/liftoff.script $src/dot.png $graphical/
      cp ${../liftoff/logo/rift-mark.png} $graphical/mark.png
      substitute $src/liftoff-graphical.plymouth $graphical/liftoff-graphical.plymouth \
        --replace-fail /usr/share/plymouth/themes/liftoff-graphical $graphical

      # the text theme: its plugin, and the two faces it draws with
      text=$out/share/plymouth/themes/liftoff-text
      mkdir -p $text $out/lib/plymouth
      cp ${fonts}/DejaVuSansMono.ttf ${fonts}/DejaVuSansMono-Bold.ttf $text/
      cp ${splash}/lib/libliftoff_splash.so $out/lib/plymouth/liftoff.so
      substitute $src/liftoff-text.plymouth $text/liftoff-text.plymouth \
        --replace-fail /usr/share/plymouth/themes/liftoff-text $text \
        --replace-fail @title@ ${lib.escapeShellArg title}
    '';
  };
in
{
  options.rift.liftoff = {
    enable = lib.mkEnableOption "the Liftoff boot splash";
    style = lib.mkOption {
      type = lib.types.enum [
        "text"
        "graphical"
      ];
      default = "text";
      description = ''
        How the boot looks: text, the console's status lines under the logo, or graphical, the
        mark. plymouth.splash=liftoff-text or liftoff-graphical on the kernel command line picks one
        for a boot.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    boot.plymouth = {
      enable = true;
      theme = "liftoff-${cfg.style}";
      themePackages = [ themes ];
      # the initrd has no gpu drivers, only the firmware framebuffer through simpledrm. plymouth
      # waits 8 s for a real drm device before it touches simpledrm unless told otherwise, and the
      # luks prompt comes before that.
      extraConfig = "UseSimpledrm=1";
    };
    # the plymouth module copies only the default theme into the initrd. both go in, so the kernel
    # command line can pick either
    boot.initrd.systemd.contents."/etc/plymouth/themes".source = lib.mkForce (
      pkgs.runCommand "plymouth-initrd-themes" { } ''
        mkdir -p $out
        cp -r ${themes}/share/plymouth/themes/. $out/
        chmod -R u+w $out
        sed -i "s,${themes}/share/plymouth/themes,$out,g" $out/*/*.plymouth
      ''
    );
    # plymouthd on the running system, which draws the shutdown, loads plugins from plymouth's own
    # directory. the text theme's plugin comes from the theme package
    environment.etc."plymouth/plugins".source = lib.mkForce (
      pkgs.symlinkJoin {
        name = "plymouth-plugins";
        paths = [
          "${config.boot.plymouth.package}/lib/plymouth"
          "${themes}/lib/plymouth"
        ];
      }
    );
    boot.kernelParams = [
      "quiet"
      "splash"
      "loglevel=3"
      # nothing on the console before plymouth starts. once it shows, plymouth turns systemd's status
      # lines on, and off again when it quits
      "rd.systemd.show_status=false"
      # a status line names the unit in bold, then its description
      "systemd.status_unit_format=combined"
      "udev.log_level=3"
    ];
    boot.consoleLogLevel = 3;
    boot.initrd.verbose = false;
  };
}
