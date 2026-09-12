//! `eclipse snapshot`: Timeline from the terminal. Lists the snapshots Vault keeps of home, takes
//! one, and restores a file from one. A file that changed since the snapshot is only replaced
//! after a yes, or with --replace.

use std::process::ExitCode;

use libeclipse::vault::{self, Refusal, Restored};

use crate::text;

const USAGE: &str = "Usage: eclipse snapshot [list]\n       eclipse snapshot take\n       \
eclipse snapshot restore [--replace] <snapshot> <file>";

const HELP: &str = "Vault takes a snapshot of your home every hour and keeps the first of each \
hour, day and week for a while. list shows them, oldest first. take takes one now. restore copies \
a file back from a snapshot. When the file has changed since, it is replaced only after you \
confirm, or at once with --replace.";

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
        Some("restore") => restore(rest),
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

fn restore(args: &[String]) -> ExitCode {
    let replace = args.iter().any(|arg| arg == "--replace");
    let words: Vec<&String> = args.iter().filter(|arg| *arg != "--replace").collect();
    if let Some(flag) = words.iter().find(|word| word.starts_with('-')) {
        return text::unknown("snapshot restore", flag, USAGE);
    }
    let [snapshot, file] = words.as_slice() else {
        eprintln!("eclipse snapshot restore needs a snapshot and a file.\n{USAGE}");
        return ExitCode::from(2);
    };
    // the service knows nothing of this shell's folder, it gets the full path
    let path = match std::path::absolute(file.as_str()) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Could not work out the full path of {file}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Some(path) = path.to_str() else {
        eprintln!("The path {} is not valid UTF-8.", path.display());
        return ExitCode::FAILURE;
    };

    let refusal = match vault::restore(snapshot, path, replace) {
        Ok((restored, written)) => {
            println!("{}", said(restored, &written, snapshot));
            return ExitCode::SUCCESS;
        }
        Err(refusal) => refusal,
    };
    let why = match refusal {
        Refusal::Changed(why) => why,
        Refusal::Other(why) => {
            eprintln!("{why}");
            return ExitCode::FAILURE;
        }
    };
    println!("{why}");
    match text::confirm("Replace it with the copy from the snapshot?") {
        Some(true) => {}
        Some(false) => {
            eprintln!("Nothing was restored.");
            return ExitCode::FAILURE;
        }
        None => {
            eprintln!("Nothing was restored. Add --replace to overwrite it with the copy.");
            return ExitCode::FAILURE;
        }
    }
    match vault::restore(snapshot, path, true) {
        Ok((restored, written)) => {
            println!("{}", said(restored, &written, snapshot));
            ExitCode::SUCCESS
        }
        Err(Refusal::Changed(why) | Refusal::Other(why)) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

fn said(restored: Restored, path: &str, snapshot: &str) -> String {
    match restored {
        Restored::Restored => format!("Restored {path} from {snapshot}."),
        Restored::Replaced => format!("Replaced {path} with the copy from {snapshot}."),
        Restored::Unchanged => {
            format!("{path} is the same as in {snapshot}. Nothing was restored.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_outcome_is_a_sentence() {
        let (path, snapshot) = ("/home/eclipse/notes.txt", "2026-09-12T14:00:03Z");
        assert_eq!(
            said(Restored::Restored, path, snapshot),
            "Restored /home/eclipse/notes.txt from 2026-09-12T14:00:03Z."
        );
        assert_eq!(
            said(Restored::Replaced, path, snapshot),
            "Replaced /home/eclipse/notes.txt with the copy from 2026-09-12T14:00:03Z."
        );
        assert_eq!(
            said(Restored::Unchanged, path, snapshot),
            "/home/eclipse/notes.txt is the same as in 2026-09-12T14:00:03Z. Nothing was restored."
        );
    }
}
