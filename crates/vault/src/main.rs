//! vault: Timeline snapshots of home. `vault serve` answers on the system bus as
//! `dev.eclipse.Vault`, `vault take` is what the hourly timer runs, and `vault prune` runs the
//! retention rules on demand. Backups and cloning come later.

mod bus;
mod restore;
mod timeline;

use std::path::PathBuf;
use std::process::ExitCode;

use timeline::{Keep, Timeline};

/// What is snapshotted, where the snapshots go, and where the snapshotted subvolume is mounted.
const SUBVOLUME: &str = "/persist/@home";
const SNAPSHOTS: &str = "/persist/@snapshots/home";
const HOME: &str = "/home";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Serve,
    Take,
    Prune,
    List,
    /// The copy a restore runs as the account that asked for it. `serve` starts it.
    RestoreFile {
        from: PathBuf,
        to: PathBuf,
    },
}

struct Args {
    command: Command,
    timeline: Timeline,
    home: PathBuf,
    replace: bool,
}

fn main() -> ExitCode {
    let Args {
        command,
        timeline,
        home,
        replace,
    } = match parse_args(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("vault: {message}");
            usage();
            return ExitCode::from(2);
        }
    };

    let result = match command {
        Command::Serve => bus::serve(timeline, home)
            .map_err(|e| format!("vault: could not answer on the system bus: {e}")),
        Command::Take => timeline.take().map(|(name, dropped)| {
            println!("Took snapshot {name}.");
            for old in dropped {
                println!("Dropped snapshot {old}.");
            }
        }),
        Command::Prune => timeline.prune().map(|dropped| {
            if dropped.is_empty() {
                println!("The rules keep every snapshot.");
            }
            for old in dropped {
                println!("Dropped snapshot {old}.");
            }
        }),
        Command::List => timeline
            .list()
            .map(|names| {
                for name in names {
                    println!("{name}");
                }
            })
            .map_err(|e| format!("Could not read {}: {e}", timeline.snapshots.display())),
        Command::RestoreFile { from, to } => {
            return match restore::copy_back(&from, &to, replace) {
                Ok(outcome) => {
                    println!("{}", outcome.name());
                    ExitCode::SUCCESS
                }
                Err(problem) => {
                    eprintln!("{}", problem.sentence());
                    ExitCode::from(problem.code())
                }
            };
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

/// `Ok(None)` means the program already did what was asked (help or version).
fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut subvolume = PathBuf::from(SUBVOLUME);
    let mut snapshots = PathBuf::from(SNAPSHOTS);
    let mut home = PathBuf::from(HOME);
    let mut keep = Keep::default();
    let mut replace = false;
    let mut words = Vec::new();
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--subvolume" => subvolume = PathBuf::from(value(&mut args, "--subvolume")?),
            "--snapshots" => snapshots = PathBuf::from(value(&mut args, "--snapshots")?),
            "--home" => home = PathBuf::from(value(&mut args, "--home")?),
            "--hourly" => keep.hourly = count(&mut args, "--hourly")?,
            "--daily" => keep.daily = count(&mut args, "--daily")?,
            "--weekly" => keep.weekly = count(&mut args, "--weekly")?,
            "--replace" => replace = true,
            "--version" | "-V" => {
                println!("vault {}", libeclipse::VERSION);
                return Ok(None);
            }
            "--help" | "-h" => {
                usage();
                return Ok(None);
            }
            flag if flag.starts_with('-') => return Err(format!("unknown argument `{flag}`")),
            _ => words.push(arg),
        }
    }
    let command = match words.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["serve"] => Command::Serve,
        ["take"] => Command::Take,
        ["prune"] => Command::Prune,
        ["list"] => Command::List,
        ["restore-file", from, to] => Command::RestoreFile {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
        },
        [] => return Err("a command is needed".into()),
        _ => return Err(format!("unknown command `{}`", words.join(" "))),
    };
    Ok(Some(Args {
        command,
        timeline: Timeline {
            subvolume,
            snapshots,
            keep,
        },
        home,
        replace,
    }))
}

fn value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{flag} needs a value"))
}

fn count(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<usize, String> {
    let text = value(args, flag)?;
    text.parse()
        .map_err(|_| format!("{flag} needs a number, not `{text}`"))
}

fn usage() {
    let keep = Keep::default();
    println!("Usage: vault <command> [options]\n");
    println!("Commands:");
    println!("  serve   Answer on the system bus as dev.eclipse.Vault and stay running");
    println!("  take    Take a snapshot of home now, then apply the retention rules");
    println!("  prune   Apply the retention rules");
    println!("  list    Print the snapshots, oldest first\n");
    println!("Options:");
    println!("  --subvolume <dir>  What is snapshotted (default {SUBVOLUME})");
    println!("  --snapshots <dir>  Where the snapshots go (default {SNAPSHOTS})");
    println!("  --home <dir>       Where that subvolume is mounted (default {HOME})");
    println!(
        "  --hourly <n>       Keep the first snapshot of this many hours (default {})",
        keep.hourly
    );
    println!(
        "  --daily <n>        And of this many days (default {})",
        keep.daily
    );
    println!(
        "  --weekly <n>       And of this many weeks (default {})",
        keep.weekly
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(list: &[&str]) -> Result<Option<Args>, String> {
        parse_args(list.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn defaults() {
        let args = parse(&["take"]).unwrap().unwrap();
        assert_eq!(args.command, Command::Take);
        assert_eq!(args.timeline.subvolume, PathBuf::from(SUBVOLUME));
        assert_eq!(args.timeline.snapshots, PathBuf::from(SNAPSHOTS));
        assert_eq!(args.timeline.keep, Keep::default());
        assert_eq!(args.home, PathBuf::from(HOME));
        assert!(!args.replace);
    }

    #[test]
    fn options() {
        let args = parse(&[
            "prune",
            "--hourly",
            "1",
            "--daily",
            "1",
            "--weekly",
            "2",
            "--snapshots",
            "/tmp/s",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(args.command, Command::Prune);
        assert_eq!(
            args.timeline.keep,
            Keep {
                hourly: 1,
                daily: 1,
                weekly: 2
            }
        );
        assert_eq!(args.timeline.snapshots, PathBuf::from("/tmp/s"));

        let args = parse(&["restore-file", "/a", "/b", "--replace"])
            .unwrap()
            .unwrap();
        assert_eq!(
            args.command,
            Command::RestoreFile {
                from: PathBuf::from("/a"),
                to: PathBuf::from("/b")
            }
        );
        assert!(args.replace);
    }

    #[test]
    fn mistakes() {
        assert!(parse(&[]).is_err());
        assert!(parse(&["take", "now"]).is_err());
        assert!(parse(&["restore-file", "/a"]).is_err());
        assert!(parse(&["prune", "--hourly"]).is_err());
        assert!(parse(&["prune", "--hourly", "many"]).is_err());
        assert!(parse(&["--bogus", "take"]).is_err());
        assert!(parse(&["--version"]).unwrap().is_none());
    }
}
