//! `eclipse --version`: the system's name and version from os-release, and with `--logo` the logo
//! above them.

use std::fs;
use std::io::{self, IsTerminal};
use std::process::ExitCode;

use libeclipse::{paths, release};

use crate::text;

const USAGE: &str = "Usage: eclipse --version [--logo]";

pub fn run(args: &[String]) -> ExitCode {
    let name = release::name(&fs::read_to_string(release::PATH).unwrap_or_default());
    match args {
        [] => {
            println!("{name}");
            ExitCode::SUCCESS
        }
        [flag] if flag == "--logo" => {
            // colours only on a terminal, and not when NO_COLOR asks for none
            let colours = io::stdout().is_terminal()
                && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
            let path = if colours {
                paths::LOGO_ANSI
            } else {
                paths::LOGO
            };
            match fs::read_to_string(path) {
                Ok(logo) => {
                    print!("{}", with_name(&logo, &name));
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("Could not read the logo at {path}: {e}.");
                    ExitCode::FAILURE
                }
            }
        }
        [flag, rest @ ..] => text::unknown("--version", rest.first().unwrap_or(flag), USAGE),
    }
}

/// The logo, a blank line, and the name under it.
fn with_name(logo: &str, name: &str) -> String {
    format!("{}\n\n{name}\n", logo.trim_end_matches('\n'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_goes_under_the_logo_after_a_blank_line() {
        assert_eq!(
            with_name("  .-.\n (   )\n  '-'\n\n", "Eclipse OS 0.1.0"),
            "  .-.\n (   )\n  '-'\n\nEclipse OS 0.1.0\n"
        );
    }
}
