//! vault: Timeline snapshots of home, and backups of it. `vault serve` answers on the system bus
//! as `dev.eclipse.Vault`, `vault take` is what the hourly timer runs, and `vault prune` runs the
//! retention rules on demand. `vault target` chooses the folder on another disk that backups go
//! to, `vault backup` makes one and `vault backups` lists them. Cloning comes later.

mod backup;
mod bus;
mod restore;
mod timeline;

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use backup::Backups;
use restore::Source;
use timeline::{Keep, Timeline};

/// What is snapshotted, where the snapshots go, and where the snapshotted subvolume is mounted.
const SUBVOLUME: &str = "/persist/@home";
const SNAPSHOTS: &str = "/persist/@snapshots/home";
const HOME: &str = "/home";
/// The backup target and its password, restores from a backup on their way, the backup disk while
/// it is mounted, and where disks are found.
const STATE: &str = "/var/lib/eclipse/vault";
const CACHE: &str = "/var/cache/vault";
const RUN: &str = "/run/vault";
const DEVICES: &str = "/dev/disk/by-uuid";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Serve,
    Take,
    Prune,
    List,
    Target {
        folder: PathBuf,
    },
    Backup,
    Backups,
    /// The copy a restore runs as the account that asked for it. `serve` starts it.
    RestoreFile {
        from: PathBuf,
        to: PathBuf,
        source: Source,
    },
}

struct Args {
    command: Command,
    timeline: Timeline,
    backups: Backups,
    home: PathBuf,
    replace: bool,
}

fn main() -> ExitCode {
    let Args {
        command,
        timeline,
        backups,
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
        Command::Serve => bus::serve(timeline, backups, home)
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
        Command::Target { folder } => {
            backups
                .choose(&folder, || ask_password(&folder))
                .map(|said| {
                    for line in said {
                        println!("{line}");
                    }
                })
        }
        Command::Backup => backups.back_up().map(|made| {
            println!(
                "Backed up home as {} at {}.",
                made.short(),
                timeline::name_of(made.time)
            );
        }),
        Command::Backups => backups.list().map(|list| {
            if list.is_empty() {
                println!("There are no backups yet.");
            }
            for made in list {
                println!("{}  {}", made.short(), timeline::name_of(made.time));
            }
        }),
        Command::RestoreFile { from, to, source } => {
            return match restore::copy_back(&from, &to, replace, source) {
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

/// The password of backups made before: typed on a terminal without echo, or one line of stdin.
fn ask_password(folder: &Path) -> Result<String, String> {
    use rustix::termios::{self, LocalModes, OptionalActions};

    let stdin = io::stdin();
    let before = if stdin.is_terminal() {
        eprint!(
            "{} holds backups already. Type their password: ",
            folder.display()
        );
        let _ = io::stderr().flush();
        let before = termios::tcgetattr(&stdin).ok();
        if let Some(mut quiet) = before.clone() {
            quiet.local_modes.remove(LocalModes::ECHO);
            let _ = termios::tcsetattr(&stdin, OptionalActions::Now, &quiet);
        }
        before
    } else {
        None
    };
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    if let Some(before) = before {
        let _ = termios::tcsetattr(&stdin, OptionalActions::Now, &before);
        eprintln!();
    }
    read.map_err(|e| format!("Could not read the password: {e}"))?;
    let password = line.trim();
    if password.is_empty() {
        Err(format!(
            "{} holds backups already, and they only open with their password.",
            folder.display()
        ))
    } else {
        Ok(password.to_string())
    }
}

/// `Ok(None)` means the program already did what was asked (help or version).
fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut subvolume = PathBuf::from(SUBVOLUME);
    let mut snapshots = PathBuf::from(SNAPSHOTS);
    let mut home = PathBuf::from(HOME);
    let mut state = PathBuf::from(STATE);
    let mut cache = PathBuf::from(CACHE);
    let mut run = PathBuf::from(RUN);
    let mut devices = PathBuf::from(DEVICES);
    let mut keep = Keep::default();
    let mut replace = false;
    let mut from_backup = false;
    let mut words = Vec::new();
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--subvolume" => subvolume = PathBuf::from(value(&mut args, "--subvolume")?),
            "--snapshots" => snapshots = PathBuf::from(value(&mut args, "--snapshots")?),
            "--home" => home = PathBuf::from(value(&mut args, "--home")?),
            "--state" => state = PathBuf::from(value(&mut args, "--state")?),
            "--cache" => cache = PathBuf::from(value(&mut args, "--cache")?),
            "--run" => run = PathBuf::from(value(&mut args, "--run")?),
            "--devices" => devices = PathBuf::from(value(&mut args, "--devices")?),
            "--hourly" => keep.hourly = count(&mut args, "--hourly")?,
            "--daily" => keep.daily = count(&mut args, "--daily")?,
            "--weekly" => keep.weekly = count(&mut args, "--weekly")?,
            "--replace" => replace = true,
            "--from-backup" => from_backup = true,
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
        ["target", folder] => Command::Target {
            folder: PathBuf::from(folder),
        },
        ["backup"] => Command::Backup,
        ["backups"] => Command::Backups,
        ["restore-file", from, to] => Command::RestoreFile {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
            source: if from_backup {
                Source::Backup
            } else {
                Source::Snapshot
            },
        },
        [] => return Err("a command is needed".into()),
        _ => return Err(format!("unknown command `{}`", words.join(" "))),
    };
    Ok(Some(Args {
        command,
        backups: Backups {
            subvolume: subvolume.clone(),
            // a backup's own snapshot goes next to Timeline's
            snapshots: snapshots.with_file_name("backup"),
            state,
            cache,
            run,
            devices,
        },
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
    println!("  serve            Answer on the system bus as dev.eclipse.Vault and stay running");
    println!("  take             Take a snapshot of home now, then apply the retention rules");
    println!("  prune            Apply the retention rules");
    println!("  list             Print the snapshots, oldest first");
    println!("  target <folder>  Back up home into this folder on another disk from now on");
    println!("  backup           Back up home now");
    println!("  backups          Print the backups, oldest first\n");
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
    println!("  --state <dir>      The backup target and its password (default {STATE})");
    println!("  --cache <dir>      Where a restore from a backup goes first (default {CACHE})");
    println!("  --run <dir>        Where the backup disk is mounted (default {RUN})");
    println!("  --devices <dir>    Where disks are found by uuid (default {DEVICES})");
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
        assert_eq!(args.backups.subvolume, PathBuf::from(SUBVOLUME));
        assert_eq!(
            args.backups.snapshots,
            PathBuf::from("/persist/@snapshots/backup")
        );
        assert_eq!(args.backups.state, PathBuf::from(STATE));
        assert_eq!(args.backups.cache, PathBuf::from(CACHE));
        assert_eq!(args.backups.run, PathBuf::from(RUN));
        assert_eq!(args.backups.devices, PathBuf::from(DEVICES));
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
            "/tmp/s/home",
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
        assert_eq!(args.timeline.snapshots, PathBuf::from("/tmp/s/home"));
        assert_eq!(args.backups.snapshots, PathBuf::from("/tmp/s/backup"));

        let args = parse(&["restore-file", "/a", "/b", "--replace"])
            .unwrap()
            .unwrap();
        assert_eq!(
            args.command,
            Command::RestoreFile {
                from: PathBuf::from("/a"),
                to: PathBuf::from("/b"),
                source: Source::Snapshot
            }
        );
        assert!(args.replace);
        let args = parse(&["restore-file", "--from-backup", "/a", "/b"])
            .unwrap()
            .unwrap();
        assert_eq!(
            args.command,
            Command::RestoreFile {
                from: PathBuf::from("/a"),
                to: PathBuf::from("/b"),
                source: Source::Backup
            }
        );

        let args = parse(&[
            "target",
            "/run/media/eclipse/Disk/Eclipse",
            "--state",
            "/tmp/v",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            args.command,
            Command::Target {
                folder: PathBuf::from("/run/media/eclipse/Disk/Eclipse")
            }
        );
        assert_eq!(args.backups.state, PathBuf::from("/tmp/v"));
        assert_eq!(
            parse(&["backups"]).unwrap().unwrap().command,
            Command::Backups
        );
        assert_eq!(
            parse(&["backup"]).unwrap().unwrap().command,
            Command::Backup
        );
    }

    #[test]
    fn mistakes() {
        assert!(parse(&[]).is_err());
        assert!(parse(&["take", "now"]).is_err());
        assert!(parse(&["restore-file", "/a"]).is_err());
        assert!(parse(&["prune", "--hourly"]).is_err());
        assert!(parse(&["prune", "--hourly", "many"]).is_err());
        assert!(parse(&["--bogus", "take"]).is_err());
        assert!(parse(&["target"]).is_err());
        assert!(parse(&["backup", "now"]).is_err());
        assert!(parse(&["backup", "--state"]).is_err());
        assert!(parse(&["--version"]).unwrap().is_none());
    }
}
