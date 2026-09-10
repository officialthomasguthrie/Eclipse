//! Aura, the fourth interpreter. A question goes to aurad on the system bus and comes back as an
//! answer in words or as the words of an OS command. Those words are read with the same parser a
//! typed line goes through, so a reply can only propose what the field already runs, and one that
//! changes something is confirmed the same way a typed one is.

// the rows are only drawn by the panel, and the panel is linux only
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::time::Duration;

use libeclipse::Component;

use crate::os::{self, Action};

/// How long an answer may take. A small model on a slow machine needs tens of seconds, and
/// aurad gives the model five minutes.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);

/// How many characters of an answer go on one row of the list. The list is as wide as the field
/// and an answer is drawn in the UI font, not in a fixed width.
pub const WIDTH: usize = 60;

/// What a reply means to the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Words to show under the field.
    Answer(String),
    /// A command Corona knows, run the way a typed one is.
    Action(Action),
    /// Nothing to show or run. Holds the sentence for the error line.
    Refused(String),
}

/// Ask aurad and wait for its kind and text.
///
/// # Errors
///
/// A sentence when the bus or Aura is not there, or Aura could not answer.
pub fn ask(question: &str) -> Result<(String, String), String> {
    let aura = Component::Aura;
    let name = aura.dbus_name();
    let connection = zbus::blocking::connection::Builder::system()
        .map(|builder| builder.method_timeout(ANSWER_TIMEOUT))
        .and_then(zbus::blocking::connection::Builder::build)
        .map_err(|e| format!("Could not reach the system bus: {e}"))?;
    let message = connection
        .call_method(
            Some(name.as_str()),
            aura.dbus_path().as_str(),
            Some(name.as_str()),
            "Ask",
            &(question,),
        )
        .map_err(|e| match e {
            zbus::Error::MethodError(error, detail, _) => reason(error.as_str(), detail.as_deref()),
            other => format!("Could not ask Aura: {other}"),
        })?;
    message
        .body()
        .deserialize::<(String, String)>()
        .map_err(|_| "Aura sent a reply Corona does not understand.".to_string())
}

/// The sentence for an error reply on the bus.
#[must_use]
pub fn reason(error: &str, detail: Option<&str>) -> String {
    match error {
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner" => "Aura is not running.".into(),
        "org.freedesktop.DBus.Error.NoReply" | "org.freedesktop.DBus.Error.Timeout" => {
            "Aura took too long to answer.".into()
        }
        _ => detail
            .map(str::trim)
            .filter(|detail| !detail.is_empty())
            .map_or_else(|| "Aura could not answer.".into(), ToString::to_string),
    }
}

/// What the field does with the two strings `Ask` returned.
#[must_use]
pub fn read(kind: &str, text: &str) -> Reply {
    match kind {
        "answer" if text.trim().is_empty() => Reply::Refused("Aura gave an empty answer.".into()),
        "answer" => Reply::Answer(text.trim().to_string()),
        "action" => {
            let words: Vec<&str> = text.split_whitespace().collect();
            match os::parse(&words) {
                Some(Ok(action)) => Reply::Action(action),
                _ => Reply::Refused(format!(
                    "Aura suggested \"{}\", which is not a command Corona runs.",
                    words.join(" ")
                )),
            }
        }
        _ => Reply::Refused("Aura sent a reply Corona does not understand.".into()),
    }
}

/// The rows an answer takes in the list: every line wrapped at spaces to `WIDTH` characters,
/// blank lines dropped, at most `limit` rows. A word longer than a row is cut into pieces.
#[must_use]
pub fn rows(answer: &str, limit: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for line in answer.lines() {
        let mut row = String::new();
        let mut used = 0;
        for word in line.split_whitespace() {
            let mut rest = word;
            loop {
                let size = rest.chars().count();
                let room = if used == 0 {
                    WIDTH
                } else {
                    WIDTH.saturating_sub(used + 1)
                };
                if size <= room {
                    if used > 0 {
                        row.push(' ');
                        used += 1;
                    }
                    row.push_str(rest);
                    used += size;
                    break;
                }
                if used > 0 {
                    rows.push(std::mem::take(&mut row));
                    used = 0;
                    continue;
                }
                let cut = rest
                    .char_indices()
                    .nth(WIDTH)
                    .map_or(rest.len(), |(at, _)| at);
                rows.push(rest[..cut].to_string());
                rest = &rest[cut..];
            }
        }
        if used > 0 {
            rows.push(row);
        }
    }
    rows.truncate(limit);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_shown() {
        assert_eq!(
            read("answer", " The capital of France is Paris.\n"),
            Reply::Answer("The capital of France is Paris.".into())
        );
        assert!(matches!(read("answer", " \n"), Reply::Refused(_)));
    }

    #[test]
    fn a_command_that_changes_something_is_one_to_confirm() {
        let Reply::Action(action) = read("action", "volume 40") else {
            panic!("volume 40 was not read as a command");
        };
        assert!(action.mutating);
        assert_eq!(action.program, "wpctl");
        assert_eq!(action.summary, "Set the volume to 40 percent");
        let Reply::Action(off) = read("action", "power  off") else {
            panic!("power off was not read as a command");
        };
        assert!(off.mutating);
        assert_eq!(off.args, ["poweroff"]);
    }

    #[test]
    fn a_command_that_only_reads_runs_at_once() {
        let Reply::Action(action) = read("action", "wifi status") else {
            panic!("wifi status was not read as a command");
        };
        assert!(!action.mutating);
        assert_eq!(action.program, "nmcli");
    }

    #[test]
    fn words_the_field_does_not_run_are_refused() {
        for words in [
            "rm -rf /",
            "wifi dance",
            "power nap",
            "systemctl poweroff",
            "",
        ] {
            let Reply::Refused(why) = read("action", words) else {
                panic!("{words:?} was not refused");
            };
            assert!(why.contains("not a command Corona runs"), "{why}");
        }
    }

    #[test]
    fn a_kind_the_field_does_not_know_is_refused() {
        assert_eq!(
            read("shell", "rm -rf /"),
            Reply::Refused("Aura sent a reply Corona does not understand.".into())
        );
    }

    #[test]
    fn errors_on_the_bus_read_as_sentences() {
        assert_eq!(
            reason("org.freedesktop.DBus.Error.ServiceUnknown", None),
            "Aura is not running."
        );
        assert_eq!(
            reason(
                "org.freedesktop.DBus.Error.NoReply",
                Some("Did not receive a reply")
            ),
            "Aura took too long to answer."
        );
        assert_eq!(
            reason(
                "org.freedesktop.DBus.Error.Failed",
                Some("The model is still loading. Try again in a moment.")
            ),
            "The model is still loading. Try again in a moment."
        );
        assert_eq!(
            reason("org.freedesktop.DBus.Error.Failed", Some(" ")),
            "Aura could not answer."
        );
    }

    #[test]
    fn a_short_answer_is_one_row() {
        assert_eq!(rows("Paris.", 8), ["Paris."]);
        assert!(rows("\n \n", 8).is_empty());
    }

    #[test]
    fn a_long_line_wraps_at_spaces() {
        let answer = "Eclipse OS runs from a USB drive and keeps everything personal on an \
                      encrypted partition, so the computer it runs on is left as it was.";
        let wrapped = rows(answer, 8);
        assert!(wrapped.len() > 1);
        assert!(wrapped.iter().all(|row| row.chars().count() <= WIDTH));
        assert_eq!(
            wrapped.join(" "),
            answer.split_whitespace().collect::<Vec<_>>().join(" ")
        );
    }

    #[test]
    fn lines_stay_rows_and_the_list_stops_at_the_limit() {
        assert_eq!(rows("One.\n\nTwo.\nThree.", 8), ["One.", "Two.", "Three."]);
        assert_eq!(rows("a\nb\nc\nd", 2), ["a", "b"]);
    }

    #[test]
    fn a_word_longer_than_a_row_is_cut() {
        let long = "x".repeat(WIDTH * 2 + 5);
        let cut = rows(&format!("see {long}"), 8);
        assert_eq!(cut[0], "see");
        assert_eq!(cut[1].chars().count(), WIDTH);
        assert_eq!(cut[2].chars().count(), WIDTH);
        assert_eq!(cut[3], "xxxxx");
    }
}
