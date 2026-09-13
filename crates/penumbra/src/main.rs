//! penumbra: sandboxes. `eclipse run --sandbox` starts `penumbra run`, which checks what the sandbox
//! may see and starts bwrap. bwrap builds the sandbox, with mounts, process ids and a user namespace
//! of its own, and starts `penumbra enter` inside it, which adds Landlock rules and a seccomp filter
//! and runs the command. Flatpak's permissions and the network switch come later.

#[cfg(target_os = "linux")]
mod confine;
mod policy;

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::{env, fs, io};

use policy::Request;

const USAGE: &str = "Usage: eclipse run --sandbox [--folder <folder>] [--read <path>]... \
[--write <path>]... <command> [<argument>]...";

const HELP: &str = "Runs a command in a sandbox. It sees the system's programs, the folder you \
are in, and nothing else of yours: not the rest of your home folder, not /persist, and not the \
computer's disks. It can change files only in that folder, and what it writes anywhere else is \
gone when it ends. It can use the network. --folder gives it another folder to run in, --read \
shows it one more file or folder read only, and --write gives it one more to change. Only what is \
inside your home folder or inside /tmp can go into a sandbox, and your home folder as a whole only \
read only.";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("run") => run(rest),
        Some("enter") => enter(rest),
        Some("--version" | "-V") => {
            println!("penumbra {}", libeclipse::VERSION);
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("penumbra runs commands in a sandbox for eclipse run --sandbox.\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> ExitCode {
    let request = match parse_run(args) {
        Ok(Some(request)) => request,
        Ok(None) => {
            println!("{USAGE}\n\n{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(why) => {
            eprintln!("eclipse run: {why}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if rustix::process::getuid().is_root() {
        eprintln!("eclipse run --sandbox runs a command as you, not as root. Run it without sudo.");
        return ExitCode::FAILURE;
    }
    match bwrap_line(&request) {
        Ok(line) => {
            let error = Command::new("bwrap").args(&line).exec();
            eprintln!("Could not run bwrap: {error}");
            ExitCode::FAILURE
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

/// The bwrap command line for a request, from this process's home, folder and mounts.
fn bwrap_line(request: &Request) -> Result<Vec<OsString>, String> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set, so it is not clear what to keep out of the sandbox.")?;
    let here = env::current_dir()
        .map_err(|error| format!("Could not tell which folder this is: {error}."))?;
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")
        .map_err(|error| format!("Could not read what is mounted: {error}."))?;
    let policy = policy::check(
        request,
        &home,
        &here,
        |path| fs::canonicalize(path),
        &policy::mount_points(&mountinfo),
    )?;
    let enter =
        env::current_exe().map_err(|error| format!("Could not find penumbra itself: {error}."))?;
    Ok(policy.bwrap(&enter, &request.command))
}

/// `penumbra run`'s arguments, or `None` when help was asked for. The first word that is not an
/// option is the command, and everything after it is the command's.
fn parse_run(args: &[String]) -> Result<Option<Request>, String> {
    let mut request = Request::default();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(None),
            "--folder" if request.folder.is_some() => {
                return Err("--folder can be given once".to_string());
            }
            "--folder" => request.folder = Some(value(&mut rest, arg)?.into()),
            "--read" => request.read.push(value(&mut rest, arg)?.into()),
            "--write" => request.write.push(value(&mut rest, arg)?.into()),
            "--" => {
                request.command = rest.cloned().collect();
                break;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown argument `{flag}`")),
            _ => {
                request.command = std::iter::once(arg).chain(rest).cloned().collect();
                break;
            }
        }
    }
    if request.command.is_empty() {
        return Err("a command is needed".to_string());
    }
    Ok(Some(request))
}

fn value<'a>(
    rest: &mut impl Iterator<Item = &'a String>,
    flag: &str,
) -> Result<&'a String, String> {
    rest.next().ok_or_else(|| format!("{flag} needs a value"))
}

/// What `penumbra enter` is given inside the sandbox.
#[derive(Debug, PartialEq, Eq)]
struct Enter {
    read: Vec<PathBuf>,
    write: Vec<PathBuf>,
    program: String,
    arguments: Vec<String>,
}

fn parse_enter(args: &[String]) -> Result<Enter, String> {
    let (mut read, mut write) = (Vec::new(), Vec::new());
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--read" => read.push(value(&mut rest, arg)?.into()),
            "--write" => write.push(value(&mut rest, arg)?.into()),
            "--" => {
                let mut command = rest.cloned();
                let program = command.next().ok_or("a command is needed after --")?;
                return Ok(Enter {
                    read,
                    write,
                    program,
                    arguments: command.collect(),
                });
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Err("a command is needed after --".to_string())
}

fn enter(args: &[String]) -> ExitCode {
    let enter = match parse_enter(args) {
        Ok(enter) => enter,
        Err(why) => {
            eprintln!("penumbra enter: {why}");
            return ExitCode::from(2);
        }
    };
    if let Err(why) = confine(&enter.read, &enter.write) {
        eprintln!("{why} Nothing was run.");
        return ExitCode::from(126);
    }
    let error = Command::new(&enter.program).args(&enter.arguments).exec();
    if error.kind() == io::ErrorKind::NotFound {
        eprintln!("{} was not found in the sandbox.", enter.program);
        ExitCode::from(127)
    } else {
        eprintln!("Could not run {}: {error}.", enter.program);
        ExitCode::from(126)
    }
}

#[cfg(target_os = "linux")]
fn confine(read: &[PathBuf], write: &[PathBuf]) -> Result<(), String> {
    confine::landlock(read, write)?;
    confine::seccomp()
}

#[cfg(not(target_os = "linux"))]
fn confine(_read: &[PathBuf], _write: &[PathBuf]) -> Result<(), String> {
    Err("A sandbox needs Linux.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn run_takes_options_then_the_command() {
        assert_eq!(
            parse_run(&args(&["--read", "/tmp/a", "ls", "--read", "-l"])),
            Ok(Some(Request {
                folder: None,
                read: vec![PathBuf::from("/tmp/a")],
                write: vec![],
                command: args(&["ls", "--read", "-l"]),
            }))
        );
        assert_eq!(
            parse_run(&args(&[
                "--folder", "src", "--write", "out", "--", "--weird"
            ])),
            Ok(Some(Request {
                folder: Some(PathBuf::from("src")),
                read: vec![],
                write: vec![PathBuf::from("out")],
                command: args(&["--weird"]),
            }))
        );
        assert_eq!(parse_run(&args(&["--help"])), Ok(None));
    }

    #[test]
    fn run_refuses_mistakes() {
        assert!(parse_run(&args(&[])).is_err());
        assert!(parse_run(&args(&["--read"])).is_err());
        assert!(parse_run(&args(&["--read", "/tmp"])).is_err());
        assert!(parse_run(&args(&["--"])).is_err());
        assert!(parse_run(&args(&["--net", "ls"])).is_err());
        assert!(parse_run(&args(&["--folder", "a", "--folder", "b", "ls"])).is_err());
    }

    #[test]
    fn enter_takes_the_rules_then_the_command() {
        assert_eq!(
            parse_enter(&args(&[
                "--read", "/usr", "--write", "/tmp", "--", "sh", "-c", "true"
            ])),
            Ok(Enter {
                read: vec![PathBuf::from("/usr")],
                write: vec![PathBuf::from("/tmp")],
                program: "sh".to_string(),
                arguments: args(&["-c", "true"]),
            })
        );
        assert!(parse_enter(&args(&["--read", "/usr"])).is_err());
        assert!(parse_enter(&args(&["--",])).is_err());
        assert!(parse_enter(&args(&["sh"])).is_err());
    }
}
