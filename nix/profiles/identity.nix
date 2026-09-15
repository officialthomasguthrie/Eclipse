# what the system calls itself and how it greets. os-release and lsb-release say Eclipse OS, the
# logo in characters heads every text console, and fastfetch shows it in the first shell of a session
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  version = config.system.image.version;
  home = "https://github.com/officialthomasguthrie/Eclipse";
  logo = import ../totality/logo { inherit lib; };
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
  # the columns fastfetch's own rows need next to a logo
  infoColumns = 60;
in
{
  # NAME, ID, ID_LIKE=nixos and DEFAULT_HOSTNAME come from the names. every other field NixOS
  # writes would carry its own release, so they are set here. IMAGE_ID and IMAGE_VERSION stay as
  # nix/image sets them, sysupdate and vault clone read them
  system.nixos = {
    distroName = "Eclipse OS";
    distroId = "eclipse";
    vendorName = "Eclipse OS";
    vendorId = "eclipse";
    extraOSReleaseArgs = {
      PRETTY_NAME = "Eclipse OS ${version}";
      VERSION = version;
      VERSION_ID = version;
      VERSION_CODENAME = "";
      BUILD_ID = self.rev or self.dirtyRev or "unknown";
      CPE_NAME = "cpe:/o:eclipse:eclipse_os:${version}";
      HOME_URL = home;
      SUPPORT_URL = "${home}/issues";
      BUG_REPORT_URL = "${home}/issues";
      LOGO = "eclipse-logo";
      # the gold of the logo
      ANSI_COLOR = "38;5;214";
    };
    extraLSBReleaseArgs = {
      LSB_VERSION = version;
      DISTRIB_RELEASE = version;
      DISTRIB_CODENAME = "";
      DISTRIB_DESCRIPTION = "Eclipse OS ${version}";
    };
  };

  # a text console shows the logo, then the name, the kernel and the console's name as Arch does,
  # then the login
  environment.etc.issue.text = ''
    ${logo.issue}
    \S{PRETTY_NAME} \r (\l)

  '';
  environment.etc."eclipse/logo.txt".text = logo.plain;
  environment.etc."eclipse/logo.ansi".text = logo.ansi;
  environment.etc."eclipse/logo-small.txt".text = logo.small.plain;
  environment.etc."eclipse/logo-small.ansi".text = logo.small.ansi;

  environment.systemPackages = [ fastfetch ];
  # fastfetch would pick the NixOS logo from ID_LIKE. the three lines at the end are Eclipse's own
  environment.etc."xdg/fastfetch/config.jsonc".text = builtins.toJSON {
    logo = {
      type = "file";
      source = "${logo.file}";
      color = lib.listToAttrs (lib.imap1 (i: c: lib.nameValuePair (toString i) c) logo.colours);
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
        text = "eclipse host class";
      }
      {
        type = "command";
        key = "AI tier";
        text = "eclipse host tier";
      }
      {
        type = "command";
        key = "Last snapshot";
        text = "eclipse snapshot last";
      }
    ];
  };

  # the first shell of a login session on a text console or in a terminal window greets with
  # fastfetch, a serial line never does. a file ~/.config/eclipse/greeting that says off turns it
  # off. the full logo goes where it fits next to fastfetch's rows, half of it where only that
  # fits, and none in a terminal narrower still
  programs.fish.interactiveShellInit = ''
    function fish_greeting
        set -q XDG_SESSION_ID XDG_RUNTIME_DIR; or return
        string match -qr '^/dev/(tty[0-9]+|pts/[0-9]+)$' -- (tty 2>/dev/null); or return
        set -l setting ~/.config/eclipse/greeting
        if test -f $setting; and string match -q off -- (string trim <$setting)
            return
        end
        set -l mark $XDG_RUNTIME_DIR/eclipse-greeted-$XDG_SESSION_ID
        test -e $mark; and return
        true >$mark
        if test $COLUMNS -ge ${
          toString (logo.columns + infoColumns)
        }; and test $LINES -ge ${toString (logo.rows + 2)}
            fastfetch
        else if test $COLUMNS -ge ${toString (logo.small.columns + infoColumns)}
            fastfetch --logo-type file --logo ${logo.small.file}
        else
            fastfetch --logo none
        end
    end
  '';
}
