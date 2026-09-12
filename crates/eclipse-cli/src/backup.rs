//! `eclipse backup`: backups of home from the terminal. Lists the backups Vault made on the backup
//! disk, makes one, and restores a file from one the way `eclipse snapshot restore` does. Choosing
//! the disk is `sudo vault target <folder>`, since the answer has the password in it.

use std::process::ExitCode;

use libeclipse::vault;

use crate::restore::{self, From};
use crate::text;

const USAGE: &str = "Usage: eclipse backup [list]\n       eclipse backup now\n       \
eclipse backup restore [--replace] <backup> <file>";

const HELP: &str = "Vault backs up your home, encrypted, into a folder on another disk, and \
mounts that disk by itself when it is plugged in. list shows the backups, oldest first. now makes \
one. restore copies a file back from a backup. When the file has changed since, it is replaced \
only after you confirm, or at once with --replace. To choose the disk, mount it and run \
sudo vault target <folder>.";

const FROM: From = From {
    command: "backup restore",
    usage: USAGE,
    noun: "backup",
    call: called,
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
        Some("now") if rest.is_empty() => now(),
        Some("restore") => restore::run(rest, &FROM, vault::restore_backup),
        Some("list" | "now") => text::unknown("backup", &rest[0], USAGE),
        Some(other) => text::unknown("backup", other, USAGE),
    }
}

fn called(id: &str) -> String {
    format!("backup {id}")
}

fn list() -> ExitCode {
    match vault::backups() {
        Ok(backups) if backups.is_empty() => {
            println!("There are no backups yet.");
            ExitCode::SUCCESS
        }
        Ok(backups) => {
            for (id, time) in backups {
                println!("{}  {time}", vault::short(&id));
            }
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

fn now() -> ExitCode {
    match vault::backup() {
        Ok((id, time)) => {
            println!("Backed up home as {} at {time}.", vault::short(&id));
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}
