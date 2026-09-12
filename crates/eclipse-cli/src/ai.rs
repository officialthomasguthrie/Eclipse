//! `eclipse ai`: a question for Aura from the terminal. An answer is printed. A command Aura
//! proposes is printed as well and follows the rule Corona's field follows: one that only reads
//! runs at once, one that changes something runs after a yes. Without a question it prints
//! Aura's state.

use std::process::ExitCode;

use libeclipse::aura::{self, Reply, Status};
use libeclipse::os::{self, Action};

use crate::text;

const USAGE: &str = "Usage: eclipse ai [--yes] [question]";

const HELP: &str = "Asks Aura a question and prints the answer. When Aura proposes a command that \
changes something, it runs only after you confirm it, or at once with --yes. Without a question, \
shows which model Aura runs and whether it is ready.";

pub fn run(args: &[String]) -> ExitCode {
    let (yes, words) = match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            println!("{USAGE}\n\n{HELP}");
            return ExitCode::SUCCESS;
        }
        Some("--yes" | "-y") => (true, &args[1..]),
        _ => (false, args),
    };
    let question = words.join(" ");
    if question.trim().is_empty() {
        return state();
    }
    let reply = match aura::ask(&question) {
        Ok((kind, text)) => aura::read(&kind, &text),
        Err(why) => Reply::Refused(why),
    };
    match reply {
        Reply::Answer(answer) => {
            println!("{answer}");
            ExitCode::SUCCESS
        }
        Reply::Action(action) => act(&action, yes),
        Reply::Refused(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

fn state() -> ExitCode {
    match aura::status() {
        Ok(status) => {
            print!("{}", text::table(&rows(&status)));
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

fn rows(status: &Status) -> Vec<(&'static str, String)> {
    let or_none = |value: &str| {
        if value.is_empty() {
            "none".to_string()
        } else {
            value.to_string()
        }
    };
    let mut rows = vec![
        ("State", status.state.clone()),
        ("Model", or_none(&status.model)),
        ("Tier", or_none(&status.tier)),
    ];
    if !status.error.is_empty() {
        rows.push(("Error", status.error.clone()));
    }
    rows
}

/// The command Aura proposed: printed first, then run, after a yes when it changes something.
fn act(action: &Action, yes: bool) -> ExitCode {
    println!("{}: {}", action.summary, text::command_line(action));
    if action.mutating && !yes {
        match text::confirm("Run this command?") {
            Some(true) => {}
            Some(false) => {
                eprintln!("Nothing was run.");
                return ExitCode::FAILURE;
            }
            None => {
                eprintln!("Nothing was run. Add --yes to run it without a question.");
                return ExitCode::FAILURE;
            }
        }
    }
    match os::run(action) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ready_model_is_three_rows() {
        let ready = Status {
            state: "ready".into(),
            model: "qwen3-0.6b-q8_0".into(),
            tier: "small".into(),
            error: String::new(),
        };
        assert_eq!(
            text::table(&rows(&ready)),
            "State: ready\nModel: qwen3-0.6b-q8_0\nTier:  small\n"
        );
    }

    #[test]
    fn without_a_model_the_reason_is_a_row() {
        let none = Status {
            state: "none".into(),
            model: String::new(),
            tier: "small".into(),
            error: "No chat model that fits this machine is on the drive.".into(),
        };
        assert_eq!(
            rows(&none),
            [
                ("State", "none".to_string()),
                ("Model", "none".to_string()),
                ("Tier", "small".to_string()),
                (
                    "Error",
                    "No chat model that fits this machine is on the drive.".to_string()
                ),
            ]
        );
    }
}
