//! penumbra: sandboxes and their network switch. `eclipse run --sandbox` starts `penumbra run`,
//! which checks what the sandbox may see and starts it in a systemd scope named for its app. In the
//! scope `penumbra start` asks Penumbra on the system bus whether the app has the network, which
//! cuts the scope's first when it does not, and starts bwrap. bwrap builds the sandbox, with mounts,
//! process ids and a user namespace of its own, and starts `penumbra enter` inside it, which adds
//! Landlock rules and a seccomp filter and runs the command. `penumbra serve` is Penumbra on the
//! bus, keeping the switch in nftables. Flatpak's permissions come later.

#[cfg(target_os = "linux")]
mod confine;
mod net;
mod policy;
mod serve;

use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::{env, fs, io};

use libeclipse::penumbra::{name_problem, scope_unit};
use policy::Request;

const USAGE: &str = "Usage: eclipse run --sandbox [--name <app>] [--folder <folder>] \
[--read <path>]... [--write <path>]... <command> [<argument>]...";

const HELP: &str = "Runs a command in a sandbox. It sees the system's programs, the folder you \
are in, and nothing else of yours: not the rest of your home folder, not /persist, and not the \
computer's disks. It can change files only in that folder, and what it writes anywhere else is \
gone when it ends. It can use the network until eclipse net off takes it away from its app. The \
app is named after the command, or --name gives it a name. --folder gives it another folder to \
run in, --read shows it one more file or folder read only, and --write gives it one more to \
change. Only what is inside your home folder or inside /tmp can go into a sandbox, and your home \
folder as a whole only read only.";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("run") => run(rest),
        Some("start") => start(rest),
        Some("enter") => enter(rest),
        Some("serve") => serve(rest),
        Some("--version" | "-V") => {
            println!("penumbra {}", libeclipse::VERSION);
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!(
                "penumbra runs commands in a sandbox for eclipse run --sandbox and keeps their \
                 network switch for eclipse net.\n{USAGE}\n       penumbra serve [--state <folder>] \
                 [--cgroups <folder>]"
            );
            ExitCode::from(2)
        }
    }
}

/// A sandbox that was asked for: the app it belongs to and what it gets.
#[derive(Debug, PartialEq, Eq)]
struct Run {
    app: String,
    request: Request,
}

fn run(args: &[String]) -> ExitCode {
    let Run { app, request } = match parse_run(args) {
        Ok(Some(run)) => run,
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
    match sandbox_line(&app, &request) {
        Ok(line) => {
            let error = Command::new("systemd-run").args(&line).exec();
            eprintln!("Could not run systemd-run: {error}");
            ExitCode::FAILURE
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

/// systemd-run's command line for a request: the app's scope, and in it `penumbra start` with the
/// bwrap line from this process's home, folder and mounts.
fn sandbox_line(app: &str, request: &Request) -> Result<Vec<OsString>, String> {
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
    let penumbra =
        env::current_exe().map_err(|error| format!("Could not find penumbra itself: {error}."))?;
    let bwrap = policy.bwrap(&penumbra, &request.command);
    Ok(scope_line(app, std::process::id(), &penumbra, bwrap))
}

/// systemd-run's arguments: a scope of the user manager in app.slice named for the app, and
/// `penumbra start` in it with bwrap's arguments.
fn scope_line(app: &str, number: u32, penumbra: &Path, bwrap: Vec<OsString>) -> Vec<OsString> {
    let unit = scope_unit(app, number);
    let mut line: Vec<OsString> = [
        "--user",
        "--scope",
        "--quiet",
        "--collect",
        "--slice",
        "app.slice",
        "--unit",
        unit.as_str(),
        "--",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    line.push(penumbra.as_os_str().to_owned());
    line.extend(["start", "--"].map(OsString::from));
    line.extend(bwrap);
    line
}

/// `penumbra run`'s arguments, or `None` when help was asked for. The first word that is not an
/// option is the command, and everything after it is the command's. The app is named after the
/// command's file when --name does not name it.
fn parse_run(args: &[String]) -> Result<Option<Run>, String> {
    let mut request = Request::default();
    let mut name: Option<String> = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(None),
            "--folder" if request.folder.is_some() => {
                return Err("--folder can be given once".to_string());
            }
            "--name" if name.is_some() => return Err("--name can be given once".to_string()),
            "--folder" => request.folder = Some(value(&mut rest, arg)?.into()),
            "--name" => name = Some(value(&mut rest, arg)?.clone()),
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
    let Some(program) = request.command.first() else {
        return Err("a command is needed".to_string());
    };
    let app = if let Some(name) = name {
        if let Some(why) = name_problem(&name) {
            return Err(why);
        }
        name
    } else {
        let file = Path::new(program)
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or(program);
        if let Some(why) = name_problem(file) {
            return Err(format!("{why} Give the sandbox one with --name."));
        }
        file.to_string()
    };
    Ok(Some(Run { app, request }))
}

fn value<'a>(
    rest: &mut impl Iterator<Item = &'a String>,
    flag: &str,
) -> Result<&'a String, String> {
    rest.next().ok_or_else(|| format!("{flag} needs a value"))
}

/// Inside the app's scope, before the sandbox is built: Penumbra cuts the scope's network when the
/// app's is off, then bwrap runs. Nothing runs when Penumbra cannot be asked.
fn start(args: &[String]) -> ExitCode {
    let Some(bwrap) = args.strip_prefix(&["--".to_string()]) else {
        eprintln!("penumbra start: bwrap's arguments are needed after --");
        return ExitCode::from(2);
    };
    match libeclipse::penumbra::starting() {
        Ok((_, true)) => {}
        Ok((app, false)) => {
            eprintln!("The network is off for {app}. eclipse net on {app} turns it on.");
        }
        Err(why) => {
            eprintln!("{why} Nothing was run.");
            return ExitCode::from(126);
        }
    }
    let error = Command::new("bwrap").args(bwrap).exec();
    eprintln!("Could not run bwrap: {error}");
    ExitCode::FAILURE
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

/// Penumbra on the system bus, with the apps that are off from the state folder.
fn serve(args: &[String]) -> ExitCode {
    let mut state = PathBuf::from(libeclipse::paths::PENUMBRA_STATE);
    let mut cgroups = PathBuf::from("/sys/fs/cgroup");
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let place = match arg.as_str() {
            "--state" => &mut state,
            "--cgroups" => &mut cgroups,
            other => {
                eprintln!("penumbra serve: unknown argument `{other}`");
                return ExitCode::from(2);
            }
        };
        match value(&mut rest, arg) {
            Ok(folder) => *place = PathBuf::from(folder),
            Err(why) => {
                eprintln!("penumbra serve: {why}");
                return ExitCode::from(2);
            }
        }
    }
    let result = serve::Switch::open(state.join(serve::OFF_FILE), cgroups).and_then(serve::serve);
    if let Err(why) = result {
        eprintln!("penumbra: {why}");
    }
    ExitCode::FAILURE
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
            Ok(Some(Run {
                app: "ls".to_string(),
                request: Request {
                    folder: None,
                    read: vec![PathBuf::from("/tmp/a")],
                    write: vec![],
                    command: args(&["ls", "--read", "-l"]),
                }
            }))
        );
        assert_eq!(
            parse_run(&args(&[
                "--folder", "src", "--write", "out", "--name", "build", "--", "--weird"
            ])),
            Ok(Some(Run {
                app: "build".to_string(),
                request: Request {
                    folder: Some(PathBuf::from("src")),
                    read: vec![],
                    write: vec![PathBuf::from("out")],
                    command: args(&["--weird"]),
                }
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
        assert!(parse_run(&args(&["--name", "a", "--name", "b", "ls"])).is_err());
        assert!(parse_run(&args(&["--name"])).is_err());
    }

    #[test]
    fn the_app_is_named_after_the_command() {
        let app = |list: &[&str]| parse_run(&args(list)).map(|run| run.map(|run| run.app));
        assert_eq!(
            app(&["/run/current-system/sw/bin/python3", "x.py"]),
            Ok(Some("python3".to_string()))
        );
        assert_eq!(app(&["yt-dlp", "-x"]), Ok(Some("yt-dlp".to_string())));
        assert_eq!(
            app(&["--name", "notes", "hx"]),
            Ok(Some("notes".to_string()))
        );
        assert_eq!(
            app(&["g++", "a.c"]),
            Err(
                "\"g++\" cannot be the name of an app. A name has up to 64 letters, digits, dots, \
                 dashes and underscores, and starts with a letter or a digit. Give the sandbox one \
                 with --name."
                    .to_string()
            )
        );
        assert!(app(&["--name", "a b", "ls"]).is_err());
        assert!(app(&["--name", "g++", "g++", "a.c"]).is_err());
    }

    #[test]
    fn the_sandbox_starts_in_a_scope_named_for_its_app() {
        assert_eq!(
            scope_line(
                "yt-dlp",
                4711,
                Path::new("/nix/store/x-eclipse/bin/penumbra"),
                vec!["--unshare-all".into(), "--".into(), "ls".into()]
            ),
            [
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "--slice",
                "app.slice",
                "--unit",
                "app-penumbra-yt-dlp-4711.scope",
                "--",
                "/nix/store/x-eclipse/bin/penumbra",
                "start",
                "--",
                "--unshare-all",
                "--",
                "ls"
            ]
            .map(OsString::from)
        );
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
