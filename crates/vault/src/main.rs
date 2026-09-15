//! vault: Timeline snapshots of home, backups of it, and clones of the whole drive. `vault serve`
//! answers on the system bus as `dev.rift.Vault`, `vault take` is what the hourly timer runs, and
//! `vault prune` runs the retention rules on demand. `vault target` chooses the folder on another
//! disk that backups go to, `vault backup` makes one and `vault backups` lists them. `vault clone`
//! writes a second drive onto a removable disk.

mod backup;
mod bus;
mod clone;
mod restore;
mod timeline;

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use backup::Backups;
use clone::Cloner;
use restore::Source;
use timeline::{Keep, Timeline};

/// What is snapshotted, where the snapshots go, and where the snapshotted subvolume is mounted.
const SUBVOLUME: &str = "/persist/@home";
const SNAPSHOTS: &str = "/persist/@snapshots/home";
const HOME: &str = "/home";
/// The backup target and its password, restores from a backup on their way, the backup disk while
/// it is mounted, and where disks are found.
const STATE: &str = "/var/lib/rift/vault";
const CACHE: &str = "/var/cache/vault";
const RUN: &str = "/run/vault";
const DEVICES: &str = "/dev/disk/by-uuid";
/// The running drive's esp, udev's names for its partitions, and where a clone's file systems are
/// mounted while they are written.
const BOOT: &str = "/boot";
const DESIGNATORS: &str = "/dev/disk/by-designator";
const CLONE_RUN: &str = "/run/vault-clone";

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
    Clone {
        disk: PathBuf,
        serial: Option<String>,
    },
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
    cloner: Cloner,
    home: PathBuf,
    replace: bool,
}

fn main() -> ExitCode {
    let Args {
        command,
        timeline,
        backups,
        cloner,
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
        Command::Clone { disk, serial } => clone_drive(&cloner, &disk, serial.as_deref()),
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

/// One line typed after `prompt` on a terminal, without echo when `hidden`, or one line of stdin
/// when there is no terminal. The line ending is not part of it.
fn read_line(prompt: &str, hidden: bool) -> Result<String, String> {
    use rustix::termios::{self, LocalModes, OptionalActions};

    let stdin = io::stdin();
    let before = if stdin.is_terminal() {
        eprint!("{prompt}");
        let _ = io::stderr().flush();
        let before = termios::tcgetattr(&stdin).ok().filter(|_| hidden);
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
    read.map_err(|e| format!("Could not read what was typed: {e}"))?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// The password of backups made before: typed on a terminal without echo, or one line of stdin.
fn ask_password(folder: &Path) -> Result<String, String> {
    let line = read_line(
        &format!(
            "{} holds backups already. Type their password: ",
            folder.display()
        ),
        true,
    )?;
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

/// Clones this drive onto `disk` once the person has seen the disk, typed its serial back and chosen
/// a passphrase.
fn clone_drive(cloner: &Cloner, disk: &Path, serial: Option<&str>) -> Result<(), String> {
    if !rustix::process::geteuid().is_root() {
        return Err(
            "A clone erases a whole disk, so it needs root. Run sudo rift clone <disk>.".into(),
        );
    }
    let plan = cloner.inspect(disk)?;
    let path = plan.disk.path.clone();
    for line in clone::describe(&plan.disk) {
        println!("{line}");
    }
    let wanted = clone::confirmation(&plan.disk);
    let terminal = io::stdin().is_terminal();
    let typed = match serial {
        Some(serial) => serial.to_string(),
        None if terminal => read_line(
            &format!("Everything on it will be erased. To go on, type its serial, {wanted}: "),
            false,
        )?,
        None => {
            return Err(
                "Everything on it would be erased. Without a terminal, give its serial with --serial."
                    .into(),
            );
        }
    };
    if typed.trim() != wanted {
        return Err(format!(
            "\"{}\" is not the serial of {path}. Nothing was written.",
            typed.trim()
        ));
    }
    let passphrase = read_line("Passphrase for the new drive: ", true)?;
    if let Some(problem) = clone::passphrase_problem(&passphrase) {
        return Err(format!("{problem} Nothing was written."));
    }
    if terminal && read_line("Type it again: ", true)? != passphrase {
        return Err("The two passphrases are not the same. Nothing was written.".into());
    }
    cloner
        .write(&plan, &passphrase, &mut |line| println!("{line}"))
        .map_err(|why| {
            format!("{why}\nThe clone did not finish, and {path} does not boot. Run it again to start over.")
        })?;
    println!(
        "{path} is a second drive now, with version {} and a copy of everything on this one but its snapshots. It opens with the new passphrase.",
        plan.running.slot.version
    );
    Ok(())
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
    let mut serial = None;
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
            "--serial" => serial = Some(value(&mut args, "--serial")?),
            "--version" | "-V" => {
                println!("vault {}", librift::VERSION);
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
        ["clone", disk] => Command::Clone {
            disk: PathBuf::from(disk),
            serial: serial.take(),
        },
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
    if serial.is_some() {
        return Err("--serial goes with clone".into());
    }
    // persist's top is where the snapshotted subvolume is
    let persist = subvolume
        .parent()
        .map_or_else(|| PathBuf::from("/persist"), Path::to_path_buf);
    Ok(Some(Args {
        command,
        backups: Backups {
            subvolume: subvolume.clone(),
            // a backup's own snapshot goes next to Timeline's, and so do a clone's
            snapshots: snapshots.with_file_name("backup"),
            state,
            cache,
            run,
            devices,
        },
        cloner: Cloner {
            persist,
            snapshots: snapshots.with_file_name("clone"),
            boot: PathBuf::from(BOOT),
            designators: PathBuf::from(DESIGNATORS),
            run: PathBuf::from(CLONE_RUN),
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
    println!("  serve            Answer on the system bus as dev.rift.Vault and stay running");
    println!("  take             Take a snapshot of home now, then apply the retention rules");
    println!("  prune            Apply the retention rules");
    println!("  list             Print the snapshots, oldest first");
    println!("  target <folder>  Back up home into this folder on another disk from now on");
    println!("  backup           Back up home now");
    println!("  backups          Print the backups, oldest first");
    println!("  clone <disk>     Erase this removable disk and write a second drive onto it\n");
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
    println!(
        "  --serial <serial>  The serial of the disk to clone onto, typed back without a terminal"
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
        assert_eq!(args.backups.subvolume, PathBuf::from(SUBVOLUME));
        assert_eq!(
            args.backups.snapshots,
            PathBuf::from("/persist/@snapshots/backup")
        );
        assert_eq!(args.backups.state, PathBuf::from(STATE));
        assert_eq!(args.backups.cache, PathBuf::from(CACHE));
        assert_eq!(args.backups.run, PathBuf::from(RUN));
        assert_eq!(args.backups.devices, PathBuf::from(DEVICES));
        assert_eq!(args.cloner.persist, PathBuf::from("/persist"));
        assert_eq!(
            args.cloner.snapshots,
            PathBuf::from("/persist/@snapshots/clone")
        );
        assert_eq!(args.cloner.boot, PathBuf::from(BOOT));
        assert_eq!(args.cloner.designators, PathBuf::from(DESIGNATORS));
        assert_eq!(args.cloner.run, PathBuf::from(CLONE_RUN));
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
        assert_eq!(args.cloner.snapshots, PathBuf::from("/tmp/s/clone"));

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

        let args = parse(&["target", "/run/media/rift/Disk/Rift", "--state", "/tmp/v"])
            .unwrap()
            .unwrap();
        assert_eq!(
            args.command,
            Command::Target {
                folder: PathBuf::from("/run/media/rift/Disk/Rift")
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
    fn a_clone_takes_one_disk_and_maybe_its_serial() {
        assert_eq!(
            parse(&["clone", "/dev/sdb"]).unwrap().unwrap().command,
            Command::Clone {
                disk: PathBuf::from("/dev/sdb"),
                serial: None
            }
        );
        assert_eq!(
            parse(&[
                "clone",
                "--serial",
                "4C530001230717117401",
                "/dev/disk/by-id/usb-SanDisk"
            ])
            .unwrap()
            .unwrap()
            .command,
            Command::Clone {
                disk: PathBuf::from("/dev/disk/by-id/usb-SanDisk"),
                serial: Some("4C530001230717117401".into())
            }
        );
        assert!(parse(&["clone"]).is_err());
        assert!(parse(&["clone", "/dev/sdb", "/dev/sdc"]).is_err());
        assert!(parse(&["clone", "/dev/sdb", "--serial"]).is_err());
        assert!(parse(&["backup", "--serial", "4C53"]).is_err());
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
