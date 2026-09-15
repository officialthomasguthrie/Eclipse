//! `rift run --sandbox`: a command in a Airlock sandbox. Airlock checks what the sandbox may
//! see, starts it in a scope named for its app and builds it, so this runs `airlock run` in its
//! place with the other arguments.

use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

const USAGE: &str = "Usage: rift run --sandbox [--name <app>] [--folder <folder>] \
[--read <path>]... [--write <path>]... <command> [<argument>]...";

pub fn run(args: &[String]) -> ExitCode {
    match airlock_args(args) {
        Ok(forwarded) => {
            let error = Command::new("airlock").args(&forwarded).exec();
            eprintln!("Could not run airlock: {error}");
            ExitCode::FAILURE
        }
        Err(why) => {
            eprintln!("rift run: {why}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// The arguments `airlock run` gets: all of them but --sandbox, which comes before the command.
fn airlock_args(args: &[String]) -> Result<Vec<String>, String> {
    let mut forwarded = vec!["run".to_string()];
    let mut sandbox = false;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--sandbox" => sandbox = true,
            "--help" | "-h" => return Ok(vec!["run".to_string(), "--help".to_string()]),
            "--name" | "--folder" | "--read" | "--write" => {
                forwarded.push(arg.clone());
                forwarded.extend(rest.next().cloned());
            }
            _ => {
                forwarded.push(arg.clone());
                forwarded.extend(rest.cloned());
                break;
            }
        }
    }
    if sandbox {
        Ok(forwarded)
    } else {
        Err("--sandbox is needed, rift run has no other way to run a command yet".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn airlock_gets_everything_but_sandbox() {
        assert_eq!(
            airlock_args(&args(&["--sandbox", "make", "-j", "4"])),
            Ok(args(&["run", "make", "-j", "4"]))
        );
        assert_eq!(
            airlock_args(&args(&[
                "--read",
                "/tmp/x",
                "--sandbox",
                "cat",
                "--sandbox"
            ])),
            Ok(args(&["run", "--read", "/tmp/x", "cat", "--sandbox"]))
        );
        assert_eq!(
            airlock_args(&args(&["--sandbox", "--name", "--sandbox", "curl"])),
            Ok(args(&["run", "--name", "--sandbox", "curl"]))
        );
        assert_eq!(
            airlock_args(&args(&["--help"])),
            Ok(args(&["run", "--help"]))
        );
    }

    #[test]
    fn without_sandbox_nothing_runs() {
        assert!(airlock_args(&args(&["make"])).is_err());
        assert!(airlock_args(&args(&[])).is_err());
        assert!(airlock_args(&args(&["--read", "--sandbox", "ls"])).is_err());
        assert!(airlock_args(&args(&["--name", "--sandbox", "ls"])).is_err());
    }
}
