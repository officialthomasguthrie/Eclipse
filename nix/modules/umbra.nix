# umbra: the compositor. greetd starts it on tty1 as the owner, with a pam session and the user's
# systemd manager, no greeter in between. the serial console keeps its own getty.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.eclipse.umbra;
  umbra = self.packages.${pkgs.stdenv.hostPlatform.system}.umbra;
  # the console: a terminal that drops down from the top of the screen over whatever is open. the
  # bind shows or hides the window with this app id, and starts ghostty with it when there is none
  console = {
    appId = "dev.eclipse.Console";
    height = 400;
  };
  # ghostty's settings, written into the owner's home once, when there is no file yet. tmpfiles
  # turns the \n into new lines
  ghosttySettings = lib.concatStringsSep "\\n" [
    "font-family = DejaVu Sans Mono"
    "font-size = 11"
    "background = #282828"
    "foreground = #d4d4d4"
    "window-theme = dark"
  ];
  # the system config. the binary still reads the niri paths: /etc/niri/config.kdl here, and a
  # file at ~/.config/niri/config.kdl replaces it for that user
  configFile = pkgs.writeText "umbra-config.kdl" ''
    input {
        keyboard {
            xkb {
            }
        }
        touchpad {
            tap
            natural-scroll
        }
    }

    layout {
        gaps 8
        background-color "${cfg.background}"
        focus-ring {
            width 2
            active-color "#78aeed"
            inactive-color "#505050"
        }
        border {
            off
        }
        default-column-width { proportion 0.5; }
    }

    prefer-no-csd

    // the console floats along the top of the working area, under corona's panel, full width
    window-rule {
        match app-id=r#"^${lib.escapeRegex console.appId}$"#
        open-floating true
        open-focused true
        default-column-width { proportion 1.0; }
        default-window-height { fixed ${toString console.height}; }
        default-floating-position x=0 y=0 relative-to="top-left"
    }
    ${lib.concatMapStringsSep "\n" (
      command: "spawn-at-startup " + lib.concatMapStringsSep " " (word: ''"${word}"'') command
    ) cfg.startup}

    hotkey-overlay {
        skip-at-startup
    }

    screenshot-path "~/Pictures/Screenshot %Y-%m-%d %H-%M-%S.png"

    binds {
        Mod+Shift+Slash hotkey-overlay-title="Show these shortcuts" { show-hotkey-overlay; }
        Mod+T hotkey-overlay-title="Open a terminal" { spawn "ghostty"; }
        Mod+Grave hotkey-overlay-title="Show or hide the console" { toggle-console app-id="${console.appId}" "${config.systemd.package}/bin/systemd-cat" "-t" "console" "ghostty" "--class=${console.appId}"; }
        Mod+Q hotkey-overlay-title="Close the window" { close-window; }
        Mod+O repeat=false hotkey-overlay-title="Show all workspaces" { toggle-overview; }

        Mod+Left { focus-column-left; }
        Mod+Right { focus-column-right; }
        Mod+Up { focus-window-up; }
        Mod+Down { focus-window-down; }
        Mod+Ctrl+Left { move-column-left; }
        Mod+Ctrl+Right { move-column-right; }
        Mod+Ctrl+Up { move-window-up; }
        Mod+Ctrl+Down { move-window-down; }
        Mod+Home { focus-column-first; }
        Mod+End { focus-column-last; }

        Mod+Page_Down { focus-workspace-down; }
        Mod+Page_Up { focus-workspace-up; }
        Mod+Ctrl+Page_Down { move-column-to-workspace-down; }
        Mod+Ctrl+Page_Up { move-column-to-workspace-up; }
        Mod+1 { focus-workspace 1; }
        Mod+2 { focus-workspace 2; }
        Mod+3 { focus-workspace 3; }
        Mod+4 { focus-workspace 4; }
        Mod+5 { focus-workspace 5; }

        Mod+Comma { consume-window-into-column; }
        Mod+Period { expel-window-from-column; }
        Mod+R hotkey-overlay-title="Cycle the column width" { switch-preset-column-width; }
        Mod+F hotkey-overlay-title="Maximize the column" { maximize-column; }
        Mod+Shift+F hotkey-overlay-title="Full screen" { fullscreen-window; }
        Mod+C { center-column; }
        Mod+Minus { set-column-width "-10%"; }
        Mod+Equal { set-column-width "+10%"; }
        Mod+V hotkey-overlay-title="Float the window" { toggle-window-floating; }
        Mod+W hotkey-overlay-title="Tabs in the column" { toggle-column-tabbed-display; }

        Print hotkey-overlay-title="Take a screenshot" { screenshot; }
        Ctrl+Print { screenshot-screen; }
        Alt+Print { screenshot-window; }

        XF86AudioRaiseVolume allow-when-locked=true { spawn "wpctl" "set-volume" "@DEFAULT_AUDIO_SINK@" "0.05+" "-l" "1.0"; }
        XF86AudioLowerVolume allow-when-locked=true { spawn "wpctl" "set-volume" "@DEFAULT_AUDIO_SINK@" "0.05-"; }
        XF86AudioMute allow-when-locked=true { spawn "wpctl" "set-mute" "@DEFAULT_AUDIO_SINK@" "toggle"; }
        XF86MonBrightnessUp allow-when-locked=true { spawn "brightnessctl" "set" "+10%"; }
        XF86MonBrightnessDown allow-when-locked=true { spawn "brightnessctl" "set" "10%-"; }

        Mod+Escape allow-inhibiting=false { toggle-keyboard-shortcuts-inhibit; }
        Mod+Shift+E hotkey-overlay-title="End the session" { quit; }
        Ctrl+Alt+Delete { quit; }
    }
  '';
in
{
  options.eclipse.umbra = {
    enable = lib.mkEnableOption "Umbra, the Eclipse compositor";
    user = lib.mkOption {
      type = lib.types.str;
      default = "eclipse";
      description = "the account the session runs as";
    };
    background = lib.mkOption {
      type = lib.types.str;
      default = "#242424";
      description = "the desktop background, a flat neutral gray. the boot test looks for it";
    };
    startup = lib.mkOption {
      type = lib.types.listOf (lib.types.listOf lib.types.str);
      default = [ ];
      example = [ [ "corona" ] ];
      description = "programs the compositor starts with the session, each as its argument list";
    };
  };

  config = lib.mkIf cfg.enable {
    services.greetd = {
      enable = true;
      # no greeter: the session command is the compositor, run as the owner. when it ends greetd
      # starts it again
      restart = true;
      # greetd drops the session's own output, systemd-cat puts umbra's log in the journal
      settings.default_session = {
        command = "${config.systemd.package}/bin/systemd-cat -t umbra ${umbra}/bin/umbra --session";
        user = cfg.user;
      };
    };
    # session files and XDG_DATA_DIRS for a greeter, nothing here reads them
    services.displayManager.enable = false;

    environment.etc."niri/config.kdl".source = configFile;
    systemd.user.tmpfiles.rules = [
      "d %h/.config/ghostty 0755 - - -"
      "f %h/.config/ghostty/config.ghostty 0644 - - - ${ghosttySettings}"
    ];
    environment.systemPackages = [
      umbra
      pkgs.ghostty
      pkgs.wl-clipboard
      pkgs.brightnessctl
    ];

    fonts.packages = [
      pkgs.noto-fonts
      pkgs.dejavu_fonts
    ];
    fonts.fontconfig.defaultFonts = {
      sansSerif = [ "Noto Sans" ];
      monospace = [ "DejaVu Sans Mono" ];
    };
  };
}
