//! horizon-lock: the lock screen.
//!
//! `horizon-lock` locks the session it runs in. Every output shows a password field on a plain gray
//! and nothing else until the owner's password unlocks it. `horizon-lock --listen` waits for
//! logind's Lock signal on its session and starts `horizon-lock` for each one, so
//! `loginctl lock-session` locks the screen too.

mod listen;
#[cfg(target_os = "linux")]
mod pam;
#[cfg(target_os = "linux")]
mod session;

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => lock(),
        Some("--listen") => listen::run(),
        Some("--version") => {
            println!("horizon-lock {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("horizon-lock: unknown option {other}. horizon-lock [--listen | --version]");
            ExitCode::from(2)
        }
    }
}

#[cfg(target_os = "linux")]
fn lock() -> ExitCode {
    match session::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("horizon-lock: {why}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn lock() -> ExitCode {
    eprintln!("horizon-lock: locking needs a Wayland session on Linux.");
    ExitCode::FAILURE
}
