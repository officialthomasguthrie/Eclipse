//! What `eclipse snapshot restore` and `eclipse backup restore` share: the arguments, the full
//! path, and asking before a file that changed is replaced.

use std::process::ExitCode;

use libeclipse::vault::{Refusal, Restored};

use crate::text;

/// Where a restore copies from, for what the command prints.
pub struct From<'a> {
    /// The command, `snapshot restore`.
    pub command: &'a str,
    pub usage: &'a str,
    /// What the name on the command line names, `snapshot`.
    pub noun: &'a str,
    /// How a sentence calls the one named, `2026-09-12T14:00:03Z` or `backup e863e83c`.
    pub call: fn(&str) -> String,
}

/// Restores from the arguments after `restore`: `[--replace] <name> <file>`. `ask` is the call to
/// Vault, with the name, the full path, and whether to replace.
pub fn run(
    args: &[String],
    from: &From,
    ask: impl Fn(&str, &str, bool) -> Result<(Restored, String), Refusal>,
) -> ExitCode {
    let replace = args.iter().any(|arg| arg == "--replace");
    let words: Vec<&String> = args.iter().filter(|arg| *arg != "--replace").collect();
    if let Some(flag) = words.iter().find(|word| word.starts_with('-')) {
        return text::unknown(from.command, flag, from.usage);
    }
    let [name, file] = words.as_slice() else {
        eprintln!(
            "eclipse {} needs a {} and a file.\n{}",
            from.command, from.noun, from.usage
        );
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
    let called = (from.call)(name);

    let refusal = match ask(name, path, replace) {
        Ok((restored, written)) => {
            println!("{}", said(restored, &written, &called));
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
    match text::confirm(&format!("Replace it with the copy from the {}?", from.noun)) {
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
    match ask(name, path, true) {
        Ok((restored, written)) => {
            println!("{}", said(restored, &written, &called));
            ExitCode::SUCCESS
        }
        Err(Refusal::Changed(why) | Refusal::Other(why)) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

fn said(restored: Restored, path: &str, from: &str) -> String {
    match restored {
        Restored::Restored => format!("Restored {path} from {from}."),
        Restored::Replaced => format!("Replaced {path} with the copy from {from}."),
        Restored::Unchanged => {
            format!("{path} is the same as in {from}. Nothing was restored.")
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
            said(Restored::Unchanged, path, "backup e863e83c"),
            "/home/eclipse/notes.txt is the same as in backup e863e83c. Nothing was restored."
        );
    }
}
