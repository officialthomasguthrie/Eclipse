# what the system calls itself and how it greets. os-release and lsb-release say Rift, and fastfetch
# shows the logo in characters in the first shell of a session, where the terminal has room for it
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  version = config.system.image.version;
  home = "https://github.com/officialthomasguthrie/Rift";
  logo = import ../liftoff/logo { inherit lib; };
  # the logo is text and the modules below are all fastfetch shows, so its image, sound, X11 and
  # desktop settings libraries stay out of the image
  fastfetch = pkgs.fastfetch.override {
    audioSupport = false;
    brightnessSupport = false;
    codecSupport = false;
    gnomeSupport = false;
    imageSupport = false;
    openclSupport = false;
    openglSupport = false;
    sqliteSupport = false;
    terminalSupport = false;
    x11Support = false;
    xfceSupport = false;
  };
  # the columns fastfetch's own rows need next to the logo
  infoColumns = 60;
in
{
  # NAME, ID, ID_LIKE=nixos and DEFAULT_HOSTNAME come from the names. every other field NixOS
  # writes would carry its own release, so they are set here. IMAGE_ID and IMAGE_VERSION stay as
  # nix/image sets them, sysupdate and vault clone read them
  system.nixos = {
    distroName = "Rift";
    distroId = "rift";
    vendorName = "Rift";
    vendorId = "rift";
    extraOSReleaseArgs = {
      PRETTY_NAME = "Rift ${version}";
      VERSION = version;
      VERSION_ID = version;
      VERSION_CODENAME = "";
      BUILD_ID = self.rev or self.dirtyRev or "unknown";
      CPE_NAME = "cpe:/o:rift:rift:${version}";
      HOME_URL = home;
      SUPPORT_URL = "${home}/issues";
      BUG_REPORT_URL = "${home}/issues";
      LOGO = "rift-logo";
      # the ice of the logo
      ANSI_COLOR = "38;2;93;172;217";
    };
    extraLSBReleaseArgs = {
      LSB_VERSION = version;
      DISTRIB_RELEASE = version;
      DISTRIB_CODENAME = "";
      DISTRIB_DESCRIPTION = "Rift ${version}";
    };
  };

  # a text console shows the name, the kernel and the console's name as Arch does, then the login.
  # the logo is not there: a console can be narrower than the logo, which is never shown cut or
  # shrunk. fish's greeting shows it after the login where it fits
  environment.etc.issue.text = ''
    \S{PRETTY_NAME} \r (\l)

  '';
  environment.etc."rift/logo.txt".text = logo.plain;
  environment.etc."rift/logo.ansi".text = logo.ansi;

  environment.systemPackages = [ fastfetch ];
  # fastfetch would pick the NixOS logo from ID_LIKE. the three lines at the end are Rift's own
  environment.etc."xdg/fastfetch/config.jsonc".text = builtins.toJSON {
    logo = {
      # every character of the logo in its own colour, as the file has them
      type = "file-raw";
      source = "${pkgs.writeText "rift-logo.ansi" logo.ansi}";
      padding.right = 3;
    };
    display = {
      # the logo keeps its own colours, the rest is the terminal's
      brightColor = false;
      color = {
        keys = "default";
        title = "default";
      };
    };
    modules = [
      "title"
      "separator"
      "os"
      "host"
      "kernel"
      "uptime"
      "packages"
      "shell"
      "display"
      {
        type = "wm";
        key = "Compositor";
      }
      "terminal"
      "cpu"
      "gpu"
      "memory"
      "disk"
      "localip"
      {
        type = "command";
        key = "Host class";
        text = "rift host class";
      }
      {
        type = "command";
        key = "AI tier";
        text = "rift host tier";
      }
      {
        type = "command";
        key = "Last snapshot";
        text = "rift snapshot last";
      }
    ];
  };

  # the first shell of a login session on a text console or in a terminal window greets with
  # fastfetch, a serial line never does. a file ~/.config/rift/greeting that says off turns it
  # off. a terminal too narrow or too short for the whole logo next to fastfetch's rows gets the
  # rows alone
  programs.fish.interactiveShellInit = ''
    function fish_greeting
        set -q XDG_SESSION_ID XDG_RUNTIME_DIR; or return
        string match -qr '^/dev/(tty[0-9]+|pts/[0-9]+)$' -- (tty 2>/dev/null); or return
        set -l setting ~/.config/rift/greeting
        if test -f $setting; and string match -q off -- (string trim <$setting)
            return
        end
        set -l mark $XDG_RUNTIME_DIR/rift-greeted-$XDG_SESSION_ID
        test -e $mark; and return
        true >$mark
        set -l wide ${toString (logo.columns + infoColumns)}
        set -l tall ${toString (logo.rows + 2)}
        if test $COLUMNS -ge $wide; and test $LINES -ge $tall
            fastfetch
        else
            fastfetch --logo none
        end
    end
  '';
}
