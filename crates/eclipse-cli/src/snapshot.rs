//! `eclipse snapshot`: Timeline from the terminal. Lists the snapshots Vault keeps of home, takes
//! one, and restores a file from one. A file that changed since the snapshot is only replaced
//! after a yes, or with --replace.

use std::process::ExitCode;

use libeclipse::vault;

use crate::restore::{self, From};
use crate::text;

const USAGE: &str = "Usage: eclipse snapshot [list]\n       eclipse snapshot take\n       \
eclipse snapshot restore [--replace] <snapshot> <file>";

const HELP: &str = "Vault takes a snapshot of your home every hour and keeps the first of each \
hour, day and week for a while. list shows them, oldest first. take takes one now. restore copies \
a file back from a snapshot. When the file has changed since, it is replaced only after you \
confirm, or at once with --replace.";

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
        Some("take") if rest.is_empty() => take(),
        Some("restore") => restore::run(rest, &FROM, vault::restore),
        Some("list" | "take") => text::unknown("snapshot", &rest[0], USAGE),
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
