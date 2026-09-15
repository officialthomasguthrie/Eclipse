//! What a line typed into the field means. Pure: no processes, no display, so it is tested
//! everywhere the workspace builds.
//!
//! The interpreters are tried in order. The four words `wifi`, `display`, `volume` and `power`
//! are reserved for the OS commands, everything else goes to the launcher first, then to the
//! shell if it reads like a pipeline, and the rest is a question for Aura.

use librift::os::{self, Action};

use crate::launcher::App;

/// One reading of the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Interpretation {
    /// Nothing typed.
    Nothing,
    /// Start an app from a desktop entry.
    Launch(App),
    /// Run an OS command, after a confirmation if it changes something.
    Os(Action),
    /// An OS command with arguments it does not understand. Holds the one-line usage.
    Usage(&'static str),
    /// A nushell pipeline.
    Shell(String),
    /// Plain words for Aura.
    Ask(String),
}

/// Words that mark a line as shell input when they come first. Short on purpose: anything a
/// person would type into a terminal without thinking.
const SHELL_WORDS: &[&str] = &[
    "ls",
    "ps",
    "sys",
    "cd",
    "cat",
    "open",
    "which",
    "du",
    "date",
    "echo",
    "print",
    "let",
    "def",
    "http",
    "git",
    "cargo",
    "nix",
    "ssh",
    "ping",
    "ip",
    "curl",
    "cp",
    "mv",
    "rm",
    "mkdir",
    "touch",
    "grep",
    "rg",
    "fd",
    "find",
    "kill",
    "top",
    "btop",
    "hx",
    "man",
    "sudo",
    "systemctl",
    "journalctl",
    "nmcli",
    "wpctl",
    "umbra",
    "rift",
];

/// Decide what `input` means against the known apps.
#[must_use]
pub fn route(input: &str, apps: &[App]) -> Interpretation {
    let line = input.trim();
    if line.is_empty() {
        return Interpretation::Nothing;
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    if let Some(parsed) = os::parse(&words) {
        return match parsed {
            Ok(action) => Interpretation::Os(action),
            Err(usage) => Interpretation::Usage(usage),
        };
    }
    if let Some(app) = matches(line, apps).into_iter().next() {
        return Interpretation::Launch(app.clone());
    }
    if looks_like_shell(line, &words) {
        return Interpretation::Shell(line.to_string());
    }
    Interpretation::Ask(line.to_string())
}

/// The apps that fit `input`, best first: the exact name, then a name that starts with it, then
/// a word of the name that starts with it, then the program it runs.
#[must_use]
pub fn matches<'a>(input: &str, apps: &'a [App]) -> Vec<&'a App> {
    let wanted = input.trim().to_lowercase();
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut ranked: Vec<(u8, &App)> = apps
        .iter()
        .filter_map(|app| {
            let name = app.name.to_lowercase();
            let program = app
                .exec
                .first()
                .map(|p| p.rsplit('/').next().unwrap_or(p).to_lowercase());
            let rank = if name == wanted {
                0
            } else if name.starts_with(&wanted) {
                1
            } else if name
                .split_whitespace()
                .any(|word| word.starts_with(&wanted))
            {
                2
            } else if program.as_deref() == Some(wanted.as_str()) {
                3
            } else {
                return None;
            };
            Some((rank, app))
        })
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    ranked.into_iter().map(|(_, app)| app).collect()
}

fn looks_like_shell(line: &str, words: &[&str]) -> bool {
    if line.contains('|') || line.contains("=>") {
        return true;
    }
    if line.starts_with(['$', '[', '{', '(', '.', '/', '~']) {
        return true;
    }
    words
        .first()
        .is_some_and(|first| SHELL_WORDS.contains(first))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apps() -> Vec<App> {
        vec![
            App {
                name: "Firefox".into(),
                exec: vec!["firefox".into()],
                terminal: false,
            },
            App {
                name: "Text editor".into(),
                exec: vec!["gnome-text-editor".into()],
                terminal: false,
            },
            App {
                name: "Helix".into(),
                exec: vec!["hx".into()],
                terminal: true,
            },
            App {
                name: "Files".into(),
                exec: vec!["nautilus".into()],
                terminal: false,
            },
        ]
    }

    fn os(input: &str) -> Action {
        match route(input, &apps()) {
            Interpretation::Os(action) => action,
            other => panic!("{input:?} routed to {other:?}"),
        }
    }

    #[test]
    fn empty_input_is_nothing() {
        assert_eq!(route("", &apps()), Interpretation::Nothing);
        assert_eq!(route("   ", &apps()), Interpretation::Nothing);
    }

    #[test]
    fn app_names_launch() {
        assert_eq!(
            route("firefox", &apps()),
            Interpretation::Launch(apps()[0].clone())
        );
        assert_eq!(
            route("Fire", &apps()),
            Interpretation::Launch(apps()[0].clone())
        );
        assert_eq!(
            route("editor", &apps()),
            Interpretation::Launch(apps()[1].clone())
        );
        assert_eq!(
            route("hx", &apps()),
            Interpretation::Launch(apps()[2].clone())
        );
    }

    #[test]
    fn best_match_comes_first() {
        let known = apps();
        let found = matches("f", &known);
        let names: Vec<&str> = found.iter().map(|app| app.name.as_str()).collect();
        assert_eq!(names, ["Files", "Firefox"]);
        assert!(matches("zzz", &known).is_empty());
    }

    #[test]
    fn os_words_beat_app_names() {
        let apps = vec![App {
            name: "Power statistics".into(),
            exec: vec!["gnome-power-statistics".into()],
            terminal: false,
        }];
        assert!(matches!(route("power off", &apps), Interpretation::Os(_)));
        assert!(matches!(route("power", &apps), Interpretation::Usage(_)));
        assert!(matches!(
            route("power statistics", &apps),
            Interpretation::Usage(_)
        ));
    }

    #[test]
    fn wifi_commands() {
        assert_eq!(
            os("wifi").args,
            ["-t", "-f", "SSID,SIGNAL,SECURITY", "device", "wifi", "list"]
        );
        assert!(!os("wifi").mutating);
        let off = os("wifi off");
        assert_eq!(off.program, "nmcli");
        assert_eq!(off.args, ["radio", "wifi", "off"]);
        assert!(off.mutating);
        assert_eq!(off.summary, "Turn wifi off");
        let connect = os("wifi connect Cafe secret");
        assert_eq!(
            connect.args,
            ["device", "wifi", "connect", "Cafe", "password", "secret"]
        );
        assert_eq!(connect.summary, "Connect to Cafe");
        assert!(matches!(
            route("wifi connect", &[]),
            Interpretation::Usage(_)
        ));
        assert!(matches!(route("wifi dance", &[]), Interpretation::Usage(_)));
    }

    #[test]
    fn volume_commands() {
        assert_eq!(os("volume").args, ["get-volume", "@DEFAULT_AUDIO_SINK@"]);
        assert_eq!(
            os("volume 40").args,
            ["set-volume", "@DEFAULT_AUDIO_SINK@", "0.40"]
        );
        assert_eq!(
            os("volume 100").args,
            ["set-volume", "@DEFAULT_AUDIO_SINK@", "1.00"]
        );
        assert_eq!(
            os("volume up").args,
            ["set-volume", "@DEFAULT_AUDIO_SINK@", "0.05+", "-l", "1.0"]
        );
        assert_eq!(
            os("volume mute").args,
            ["set-mute", "@DEFAULT_AUDIO_SINK@", "1"]
        );
        assert!(os("volume 40").mutating);
        assert!(matches!(route("volume 140", &[]), Interpretation::Usage(_)));
        assert!(matches!(
            route("volume loud", &[]),
            Interpretation::Usage(_)
        ));
    }

    #[test]
    fn display_commands() {
        assert_eq!(os("display").program, "brightnessctl");
        assert_eq!(os("display brightness 70").args, ["set", "70%"]);
        assert_eq!(os("display brightness +10").args, ["set", "+10%"]);
        assert_eq!(os("display brightness -10").args, ["set", "10%-"]);
        assert_eq!(os("display outputs").program, "umbra");
        assert!(!os("display outputs").mutating);
        assert!(matches!(
            route("display brightness bright", &[]),
            Interpretation::Usage(_)
        ));
    }

    #[test]
    fn power_commands() {
        assert_eq!(os("power off").args, ["poweroff"]);
        assert_eq!(os("power reboot").args, ["reboot"]);
        assert_eq!(os("power suspend").args, ["suspend"]);
        assert!(os("power off").mutating);
        assert_eq!(os("power off").summary, "Turn the computer off");
        assert!(matches!(route("power nap", &[]), Interpretation::Usage(_)));
    }

    #[test]
    fn pipelines_go_to_the_shell() {
        assert_eq!(
            route("ps | where cpu > 50", &apps()),
            Interpretation::Shell("ps | where cpu > 50".into())
        );
        assert_eq!(
            route("ls ~/Documents", &apps()),
            Interpretation::Shell("ls ~/Documents".into())
        );
        assert_eq!(
            route("$env.PATH", &apps()),
            Interpretation::Shell("$env.PATH".into())
        );
        assert_eq!(
            route("git status", &apps()),
            Interpretation::Shell("git status".into())
        );
    }

    #[test]
    fn the_rest_is_a_question() {
        assert_eq!(
            route("how much disk is left", &apps()),
            Interpretation::Ask("how much disk is left".into())
        );
        assert_eq!(
            route("  what time is it in Tokyo ", &apps()),
            Interpretation::Ask("what time is it in Tokyo".into())
        );
    }
}
