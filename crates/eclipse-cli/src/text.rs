//! How the commands put things on the terminal: rows of a label and a value, commands the way a
//! person would type them, sizes in binary units.

use std::fmt::Write as _;
use std::io::{self, BufRead, IsTerminal, Write as _};
use std::process::ExitCode;

use libeclipse::os::Action;

/// `Label: value` rows with the values lined up one space past the longest label.
pub fn table(rows: &[(&str, String)]) -> String {
    let width = rows
        .iter()
        .map(|(label, _)| label.chars().count() + 1)
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for (label, value) in rows {
        let label = format!("{label}:");
        let _ = writeln!(out, "{label:<width$} {value}");
    }
    out
}

/// An argument as it would be typed into a shell: bare when it is plain, in single quotes when
/// it is not.
pub fn quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@%+=:,./_-".contains(c));
    if plain {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// The program and its arguments on one line.
pub fn command_line(action: &Action) -> String {
    let mut line = action.program.to_string();
    for arg in &action.args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    line
}

/// Bytes in GiB with one decimal, or in MiB below that.
pub fn size(bytes: u64) -> String {
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    if bytes >= GIB - MIB / 2 {
        let gib = u128::from(GIB);
        let tenths = (u128::from(bytes) * 10 + gib / 2) / gib;
        format!("{}.{} GiB", tenths / 10, tenths % 10)
    } else {
        format!("{} MiB", bytes.div_ceil(MIB))
    }
}

/// The first 12 characters of a hash, which is enough to tell two apart.
pub fn short(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

/// What a command says about an argument it does not take.
pub fn unknown(command: &str, arg: &str, usage: &str) -> ExitCode {
    eprintln!("eclipse {command}: unknown argument `{arg}`\n{usage}");
    ExitCode::from(2)
}

/// Asks a question on the terminal with `[y/N]` after it. `None` when there is no terminal to
/// ask on.
pub fn confirm(question: &str) -> Option<bool> {
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        return None;
    }
    print!("{question} [y/N] ");
    io::stdout().flush().ok()?;
    let mut line = String::new();
    stdin.lock().read_line(&mut line).ok()?;
    Some(agrees(&line))
}

fn agrees(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_line_up_past_the_longest_label() {
        let rows = [
            ("Fingerprint", "5297c0f65d6a".to_string()),
            ("AI tier", "small".to_string()),
        ];
        assert_eq!(
            table(&rows),
            "Fingerprint: 5297c0f65d6a\nAI tier:     small\n"
        );
        assert_eq!(table(&[]), "");
    }

    #[test]
    fn plain_arguments_stay_bare_and_the_rest_are_quoted() {
        assert_eq!(quote("@DEFAULT_AUDIO_SINK@"), "@DEFAULT_AUDIO_SINK@");
        assert_eq!(quote("0.05+"), "0.05+");
        assert_eq!(quote("Cafe Wifi"), "'Cafe Wifi'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn a_proposed_command_reads_as_typed() {
        let action = Action {
            program: "nmcli",
            args: vec![
                "device".into(),
                "wifi".into(),
                "connect".into(),
                "Cafe Wifi".into(),
            ],
            mutating: true,
            summary: "Connect to Cafe Wifi".into(),
        };
        assert_eq!(
            command_line(&action),
            "nmcli device wifi connect 'Cafe Wifi'"
        );
    }

    #[test]
    fn sizes_are_in_binary_units() {
        assert_eq!(size(0), "0 MiB");
        assert_eq!(size(640 * (1 << 20)), "640 MiB");
        assert_eq!(size((1 << 30) - 1), "1.0 GiB");
        assert_eq!(size(2_147_483_648), "2.0 GiB");
        assert_eq!(size(1_288_490_189), "1.2 GiB");
    }

    #[test]
    fn only_a_yes_is_a_yes() {
        for line in ["y\n", "Y", " yes \n", "YES"] {
            assert!(agrees(line), "{line:?}");
        }
        for line in ["", "\n", "n", "no", "sure", "yes please"] {
            assert!(!agrees(line), "{line:?}");
        }
    }

    #[test]
    fn a_hash_is_cut_to_twelve() {
        assert_eq!(short("5297c0f65d6a0123456789"), "5297c0f65d6a");
        assert_eq!(short("abc"), "abc");
    }
}
