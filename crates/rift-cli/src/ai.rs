//! `rift ai`: a question for Quasar from the terminal. An answer is printed. A command Quasar
//! proposes is printed as well and follows the rule Lens's field follows: one that only reads
//! runs at once, one that changes something runs after a yes. Without a question it prints
//! Quasar's state. `rift ai index` and `rift ai search` are search by meaning in home.

use std::process::ExitCode;

use librift::os::{self, Action};
use librift::quasar::{self, Reply, Status};

use crate::{search, text};

const USAGE: &str = "Usage: rift ai [--yes] [question]
       rift ai index
       rift ai search <words>";

const HELP: &str =
    "Asks Quasar a question and prints the answer. When Quasar proposes a command that \
changes something, it runs only after you confirm it, or at once with --yes. Without a question, \
shows which models Quasar runs and whether they are ready.

  index    bring the search index of your home folder up to date. It also runs every 15 minutes.
  search   list the files in your home folder closest in meaning to the words, best first.";

pub fn run(args: &[String]) -> ExitCode {
    let (yes, words) = match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            println!("{USAGE}\n\n{HELP}");
            return ExitCode::SUCCESS;
        }
        Some("index") => return search::index(&args[1..]),
        Some("search") => return search::search(&args[1..]),
        Some("--yes" | "-y") => (true, &args[1..]),
        _ => (false, args),
    };
    let question = words.join(" ");
    if question.trim().is_empty() {
        return state();
    }
    let reply = match quasar::ask(&question) {
        Ok((kind, text)) => quasar::read(&kind, &text),
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
    match quasar::status() {
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
    let search = if status.embedding_model.is_empty() {
        status.embedding_state.clone()
    } else {
        format!("{}, {}", status.embedding_state, status.embedding_model)
    };
    rows.push(("Search", search));
    if !status.embedding_error.is_empty() {
        rows.push(("Search error", status.embedding_error.clone()));
    }
    rows
}

/// The command Quasar proposed: printed first, then run, after a yes when it changes something.
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
    fn ready_models_are_four_rows() {
        let ready = Status {
            state: "ready".into(),
            model: "qwen3-0.6b-q8_0".into(),
            tier: "small".into(),
            error: String::new(),
            embedding_state: "ready".into(),
            embedding_model: "nomic-embed-text-v1.5-q8".into(),
            embedding_error: String::new(),
        };
        assert_eq!(
            text::table(&rows(&ready)),
            "State:  ready\nModel:  qwen3-0.6b-q8_0\nTier:   small\n\
             Search: ready, nomic-embed-text-v1.5-q8\n"
        );
    }

    #[test]
    fn without_a_model_the_reason_is_a_row() {
        let none = Status {
            state: "none".into(),
            model: String::new(),
            tier: "small".into(),
            error: "No chat model that fits this machine is on the drive.".into(),
            embedding_state: "none".into(),
            embedding_model: String::new(),
            embedding_error:
                "Search by meaning needs nomic-embed-text-v1.5.Q8_0.gguf, which is not on the drive."
                    .into(),
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
                ("Search", "none".to_string()),
                (
                    "Search error",
                    "Search by meaning needs nomic-embed-text-v1.5.Q8_0.gguf, which is not on \
                     the drive."
                        .to_string()
                ),
            ]
        );
    }
}
