//! `rift snapshot`: Timeline from the terminal. Lists the snapshots Vault keeps of home, says how
//! long ago the newest was taken, takes one, and restores a file from one. A file that changed
//! since the snapshot is only replaced after a yes, or with --replace.

use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use librift::vault;

use crate::restore::{self, From};
use crate::text;

const USAGE: &str = "Usage: rift snapshot [list]\n       rift snapshot last\n       \
rift snapshot take\n       rift snapshot restore [--replace] <snapshot> <file>";

const HELP: &str = "Vault takes a snapshot of your home every hour and keeps the first of each \
hour, day and week for a while. list shows them, oldest first. last says how long ago the newest \
was taken. take takes one now. restore copies a file back from a snapshot. When the file has \
changed since, it is replaced only after you confirm, or at once with --replace.";

const FROM: From = From {
    command: "snapshot restore",
    usage: USAGE,
    noun: "snapshot",
    call: str::to_string,
};

pub fn run(args: &[String]) -> ExitCode {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            println!("{USAGE}\n\n{HELP}");
            ExitCode::SUCCESS
        }
        None => list(),
        Some("list") if rest.is_empty() => list(),
        Some("last") if rest.is_empty() => last(),
        Some("take") if rest.is_empty() => take(),
        Some("restore") => restore::run(rest, &FROM, vault::restore),
        Some("list" | "last" | "take") => text::unknown("snapshot", &rest[0], USAGE),
        Some(other) => text::unknown("snapshot", other, USAGE),
    }
}

fn list() -> ExitCode {
    match vault::list() {
        Ok(names) if names.is_empty() => {
            println!("There are no snapshots yet.");
            ExitCode::SUCCESS
        }
        Ok(names) => {
            for name in names {
                println!("{name}");
            }
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

fn last() -> ExitCode {
    match vault::list() {
        Ok(names) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|since| i64::try_from(since.as_secs()).ok())
                .unwrap_or(0);
            println!("{}", newest(&names, now));
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

fn take() -> ExitCode {
    match vault::take() {
        Ok(name) => {
            println!("Took snapshot {name}.");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

/// How long before `now` the newest of the snapshots was taken, both in seconds since 1970.
fn newest(names: &[String], now: i64) -> String {
    match names
        .iter()
        .filter_map(|name| vault::snapshot_time(name))
        .max()
    {
        Some(taken) => ago(now - taken),
        None => "none yet".to_string(),
    }
}

/// A span of seconds the way a person says it about the past.
fn ago(seconds: i64) -> String {
    let (count, unit) = match seconds {
        ..60 => return "just now".to_string(),
        60..3600 => (seconds / 60, "minute"),
        3600..172_800 => (seconds / 3600, "hour"),
        _ => (seconds / 86_400, "day"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_says_how_long_ago_the_newest_snapshot_was_taken() {
        let taken: Vec<String> = [
            "2026-09-15T09:00:00Z",
            "2026-09-15T11:48:00Z",
            "not a snapshot",
        ]
        .iter()
        .map(|name| (*name).to_string())
        .collect();
        let at = |name| vault::snapshot_time(name).unwrap_or_default();
        assert_eq!(newest(&taken, at("2026-09-15T12:00:00Z")), "12 minutes ago");
        assert_eq!(newest(&taken, at("2026-09-15T11:48:30Z")), "just now");
        assert_eq!(newest(&taken, at("2026-09-15T12:48:00Z")), "1 hour ago");
        assert_eq!(newest(&taken, at("2026-09-17T11:47:00Z")), "47 hours ago");
        assert_eq!(newest(&taken, at("2026-09-18T12:00:00Z")), "3 days ago");
        assert_eq!(newest(&[], at("2026-09-15T12:00:00Z")), "none yet");
    }

    #[test]
    fn a_clock_behind_the_snapshot_says_just_now() {
        assert_eq!(ago(-120), "just now");
        assert_eq!(ago(60), "1 minute ago");
    }
}
