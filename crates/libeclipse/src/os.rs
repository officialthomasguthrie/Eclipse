//! The OS commands: `wifi`, `display`, `volume`, `power`. Each one is planned as a program
//! with arguments and run as that program, never through a shell. Corona's field and the eclipse
//! command both plan them here. The system tools do the work for now: nmcli, brightnessctl,
//! wpctl, systemctl and umbra's own ipc. The D-Bus calls the decision record asks for come with
//! the services that will answer them.

use std::process::Command;

/// A program to run, with what it does in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// The program, looked up in PATH.
    pub program: &'static str,
    /// Its arguments.
    pub args: Vec<String>,
    /// True when it changes something. Whoever runs it asks first.
    pub mutating: bool,
    /// What it does, as a short sentence without a full stop: "Turn wifi off".
    pub summary: String,
}

impl Action {
    fn read(program: &'static str, args: &[&str], summary: &str) -> Self {
        Self {
            program,
            args: args.iter().map(ToString::to_string).collect(),
            mutating: false,
            summary: summary.to_string(),
        }
    }

    fn change(program: &'static str, args: &[&str], summary: &str) -> Self {
        Self {
            mutating: true,
            ..Self::read(program, args, summary)
        }
    }
}

const WIFI: &str = "wifi: list, status, on, off, connect <network> [password]";
const DISPLAY: &str = "display: brightness [<percent>, +10, -10], outputs";
const VOLUME: &str = "volume: <0 to 100>, up, down, mute, unmute";
const POWER: &str = "power: off, reboot, suspend";

/// Read the words of a line as an OS command. `None` when the first word is not one of the
/// four, `Err` with the usage line when the arguments make no sense.
#[must_use]
pub fn parse(words: &[&str]) -> Option<Result<Action, &'static str>> {
    let (first, rest) = words.split_first()?;
    let parsed = match *first {
        "wifi" => wifi(rest),
        "display" => display(rest),
        "volume" => volume(rest),
        "power" => power(rest),
        _ => return None,
    };
    Some(parsed)
}

fn wifi(args: &[&str]) -> Result<Action, &'static str> {
    match args {
        [] | ["list"] => Ok(Action::read(
            "nmcli",
            &["-t", "-f", "SSID,SIGNAL,SECURITY", "device", "wifi", "list"],
            "List wifi networks",
        )),
        ["status"] => Ok(Action::read(
            "nmcli",
            &["-t", "-f", "DEVICE,STATE,CONNECTION", "device", "status"],
            "Show the network status",
        )),
        ["on"] => Ok(Action::change(
            "nmcli",
            &["radio", "wifi", "on"],
            "Turn wifi on",
        )),
        ["off"] => Ok(Action::change(
            "nmcli",
            &["radio", "wifi", "off"],
            "Turn wifi off",
        )),
        ["connect", network] => Ok(Action::change(
            "nmcli",
            &["device", "wifi", "connect", network],
            &format!("Connect to {network}"),
        )),
        ["connect", network, password] => Ok(Action::change(
            "nmcli",
            &["device", "wifi", "connect", network, "password", password],
            &format!("Connect to {network}"),
        )),
        _ => Err(WIFI),
    }
}

fn display(args: &[&str]) -> Result<Action, &'static str> {
    match args {
        [] | ["brightness"] => Ok(Action::read(
            "brightnessctl",
            &["-m"],
            "Show the brightness",
        )),
        ["brightness", value] => {
            let step = value
                .strip_prefix('+')
                .map(|n| (n, "+"))
                .or_else(|| value.strip_prefix('-').map(|n| (n, "-")));
            let (number, sign) = step.unwrap_or((value, ""));
            let percent: u8 = number.parse().map_err(|_| DISPLAY)?;
            if percent > 100 {
                return Err(DISPLAY);
            }
            let setting = match sign {
                "+" => format!("+{percent}%"),
                "-" => format!("{percent}%-"),
                _ => format!("{percent}%"),
            };
            Ok(Action::change(
                "brightnessctl",
                &["set", &setting],
                &format!("Set the brightness to {setting}"),
            ))
        }
        ["outputs"] => Ok(Action::read(
            "umbra",
            &["msg", "outputs"],
            "List the displays",
        )),
        _ => Err(DISPLAY),
    }
}

fn volume(args: &[&str]) -> Result<Action, &'static str> {
    const SINK: &str = "@DEFAULT_AUDIO_SINK@";
    match args {
        [] => Ok(Action::read(
            "wpctl",
            &["get-volume", SINK],
            "Show the volume",
        )),
        ["up"] => Ok(Action::change(
            "wpctl",
            &["set-volume", SINK, "0.05+", "-l", "1.0"],
            "Turn the volume up",
        )),
        ["down"] => Ok(Action::change(
            "wpctl",
            &["set-volume", SINK, "0.05-"],
            "Turn the volume down",
        )),
        ["mute"] => Ok(Action::change(
            "wpctl",
            &["set-mute", SINK, "1"],
            "Mute the sound",
        )),
        ["unmute"] => Ok(Action::change(
            "wpctl",
            &["set-mute", SINK, "0"],
            "Unmute the sound",
        )),
        [level] => {
            let percent: u8 = level.parse().map_err(|_| VOLUME)?;
            if percent > 100 {
                return Err(VOLUME);
            }
            let fraction = format!("{}.{:02}", percent / 100, percent % 100);
            Ok(Action::change(
                "wpctl",
                &["set-volume", SINK, &fraction],
                &format!("Set the volume to {percent} percent"),
            ))
        }
        _ => Err(VOLUME),
    }
}

fn power(args: &[&str]) -> Result<Action, &'static str> {
    match args {
        ["off"] => Ok(Action::change(
            "systemctl",
            &["poweroff"],
            "Turn the computer off",
        )),
        ["reboot"] => Ok(Action::change(
            "systemctl",
            &["reboot"],
            "Restart the computer",
        )),
        ["suspend"] => Ok(Action::change(
            "systemctl",
            &["suspend"],
            "Put the computer to sleep",
        )),
        _ => Err(POWER),
    }
}

/// Run the action and wait for it. Ok holds its output, Err one line on what went wrong.
///
/// # Errors
///
/// When the program is missing, cannot start, or exits with a failure.
pub fn run(action: &Action) -> Result<String, String> {
    let output = Command::new(action.program)
        .args(&action.args)
        .output()
        .map_err(|e| format!("Could not run {}: {e}", action.program))?;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_string();
    if output.status.success() {
        Ok(text(&output.stdout))
    } else {
        let why = text(&output.stderr);
        let why = why.lines().next().unwrap_or("");
        if why.is_empty() {
            Err(format!("{} failed ({})", action.program, output.status))
        } else {
            Err(why.to_string())
        }
    }
}
