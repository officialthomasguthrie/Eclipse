//! lens: the shell. One field on the desktop and four interpreters behind it: the app
//! launcher, the OS commands, nushell and Quasar.
//!
//! `lens` draws the field as a layer-shell panel on the running session. `lens --route
//! <words>` prints what the field would do with those words and runs nothing. `lens --do
//! [--yes] <words>` does it from a terminal instead, with `--yes` standing in for the
//! confirmation the field asks for. `lens --type <words>`, `lens --enter [<words>]` and
//! `lens --escape` type into the field of the panel that is already running.

mod answer;
mod control;
mod launcher;
mod nu;
mod route;
#[cfg(target_os = "linux")]
mod ui;

use std::env;
use std::process::ExitCode;

use librift::{os, quasar};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("lens {}", librift::VERSION);
            ExitCode::SUCCESS
        }
        Some("--route") => {
            let apps = launcher::load();
            println!("{:?}", route::route(&args[1..].join(" "), &apps));
            ExitCode::SUCCESS
        }
        Some("--do") => {
            let yes = args.get(1).is_some_and(|a| a == "--yes");
            let words = &args[if yes { 2 } else { 1 }..];
            act(&words.join(" "), yes)
        }
        Some("--type") => tell(&control::Command::Type(args[1..].join(" "))),
        Some("--enter") => tell(&control::Command::Enter(args[1..].join(" "))),
        Some("--escape") => tell(&control::Command::Escape),
        Some(other) => {
            eprintln!(
                "lens: unknown option {other}. lens [--version | --route <words> | --do [--yes] <words> | --type <words> | --enter [<words>] | --escape]"
            );
            ExitCode::from(2)
        }
        None => panel(),
    }
}

/// Type into the field of the panel that is running.
fn tell(command: &control::Command) -> ExitCode {
    match control::send(command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

/// What the field does on Enter, from a terminal.
fn act(input: &str, yes: bool) -> ExitCode {
    use route::Interpretation;
    let apps = launcher::load();
    let outcome = match route::route(input, &apps) {
        Interpretation::Nothing => Ok(String::new()),
        Interpretation::Launch(app) => {
            launcher::launch(&app).map(|()| format!("Starting {}", app.name))
        }
        Interpretation::Os(action) => command(&action, yes),
        Interpretation::Usage(usage) => Err(usage.to_string()),
        Interpretation::Shell(line) => nu::run(&line),
        Interpretation::Ask(question) => {
            quasar::ask(&question).and_then(|(kind, text)| match quasar::read(&kind, &text) {
                quasar::Reply::Answer(answer) => Ok(answer),
                quasar::Reply::Action(action) => command(&action, yes),
                quasar::Reply::Refused(why) => Err(why),
            })
        }
    };
    match outcome {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

/// An OS command from a terminal, typed or proposed by Quasar. One that changes something waits
/// for `--yes`.
fn command(action: &os::Action, yes: bool) -> Result<String, String> {
    if action.mutating && !yes {
        Err(format!("{}? Run again with --yes.", action.summary))
    } else {
        os::run(action)
    }
}

#[cfg(target_os = "linux")]
fn panel() -> ExitCode {
    let apps = launcher::load();
    eprintln!(
        "lens {}: {} apps, opening the panel",
        librift::VERSION,
        apps.len()
    );
    match ui::run(apps) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lens: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn panel() -> ExitCode {
    eprintln!("lens: the panel needs a Wayland session on Linux");
    ExitCode::FAILURE
}
