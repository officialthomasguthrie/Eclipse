//! corona: the shell. One field on the desktop and four interpreters behind it: the app
//! launcher, the OS commands, nushell and Aura. The first two work, the last two say so.
//!
//! `corona` draws the field as a layer-shell panel on the running session. `corona --route
//! <words>` prints what the field would do with those words and runs nothing. `corona --do
//! [--yes] <words>` does it from a terminal instead, with `--yes` standing in for the
//! confirmation the field asks for.

mod launcher;
mod os;
mod route;
#[cfg(target_os = "linux")]
mod ui;

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("corona {}", libeclipse::VERSION);
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
        Some(other) => {
            eprintln!(
                "corona: unknown option {other}. corona [--version | --route <words> | --do [--yes] <words>]"
            );
            ExitCode::from(2)
        }
        None => panel(),
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
        Interpretation::Os(action) if action.mutating && !yes => {
            Err(format!("{}? Run again with --yes.", action.summary))
        }
        Interpretation::Os(action) => os::run(&action),
        Interpretation::Usage(usage) => Err(usage.to_string()),
        Interpretation::Shell(_) => Err("Nushell is not in this build yet.".to_string()),
        Interpretation::Ask(_) => Err("Aura is not in this build yet.".to_string()),
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

#[cfg(target_os = "linux")]
fn panel() -> ExitCode {
    let apps = launcher::load();
    eprintln!(
        "corona {}: {} apps, opening the panel",
        libeclipse::VERSION,
        apps.len()
    );
    match ui::run(apps) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("corona: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn panel() -> ExitCode {
    eprintln!("corona: the panel needs a Wayland session on Linux");
    ExitCode::FAILURE
}
