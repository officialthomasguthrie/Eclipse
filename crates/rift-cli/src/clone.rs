//! `rift clone`: a second drive, written onto a removable disk. Vault does the work as root, so
//! this runs `vault clone` in its place with the same arguments.

use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

const USAGE: &str = "Usage: sudo rift clone [--serial <serial>] <disk>";

const HELP: &str = "Writes a complete second drive onto a removable or USB disk: the system that \
runs now, an empty slot for the next update, and everything on persist, encrypted with a new key \
and a passphrase you choose. Timeline snapshots stay on this drive. Everything on the disk is \
erased, so Vault shows the disk first and asks you to type its serial. It refuses the drive this \
system runs from, a disk inside the computer, and a disk that is in use. Without a terminal, give \
the serial with --serial and the passphrase on one line of input.";

pub fn run(args: &[String]) -> ExitCode {
    match vault_args(args) {
        Ok(None) => {
            println!("{USAGE}\n\n{HELP}");
            ExitCode::SUCCESS
        }
        Ok(Some(forwarded)) => {
            let error = Command::new("vault").args(&forwarded).exec();
            eprintln!("Could not run vault: {error}");
            ExitCode::FAILURE
        }
        Err(why) => {
            eprintln!("rift clone: {why}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// The arguments `vault clone` gets, or `None` when help was asked for.
fn vault_args(args: &[String]) -> Result<Option<Vec<String>>, String> {
    let mut serial = None;
    let mut disk = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(None),
            "--serial" => serial = Some(rest.next().ok_or("--serial needs a value")?.clone()),
            flag if flag.starts_with('-') => return Err(format!("unknown argument `{flag}`")),
            _ if disk.is_some() => {
                return Err(format!("one disk at a time, `{arg}` is a second one"));
            }
            _ => disk = Some(arg.clone()),
        }
    }
    let disk = disk.ok_or("a disk is needed")?;
    let mut forwarded = vec!["clone".to_string()];
    if let Some(serial) = serial {
        forwarded.extend(["--serial".to_string(), serial]);
    }
    forwarded.push(disk);
    Ok(Some(forwarded))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn vault_gets_the_disk_and_the_serial() {
        assert_eq!(
            vault_args(&args(&["/dev/sdb"])),
            Ok(Some(args(&["clone", "/dev/sdb"])))
        );
        assert_eq!(
            vault_args(&args(&["/dev/disk/by-id/usb-Stick", "--serial", "4C53"])),
            Ok(Some(args(&[
                "clone",
                "--serial",
                "4C53",
                "/dev/disk/by-id/usb-Stick"
            ])))
        );
        assert_eq!(vault_args(&args(&["--help"])), Ok(None));
    }

    #[test]
    fn mistakes_are_refused_before_vault_runs() {
        assert!(vault_args(&args(&[])).is_err());
        assert!(vault_args(&args(&["--serial"])).is_err());
        assert!(vault_args(&args(&["--serial", "4C53"])).is_err());
        assert!(vault_args(&args(&["/dev/sdb", "/dev/sdc"])).is_err());
        assert!(vault_args(&args(&["--yes", "/dev/sdb"])).is_err());
    }
}
