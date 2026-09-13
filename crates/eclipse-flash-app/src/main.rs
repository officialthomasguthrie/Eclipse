//! The eclipse-flash app: a window that picks an image and a stick, has the stick's serial or name
//! typed back, and starts `eclipse-flash write` with the rights to erase the stick, showing the steps
//! it prints. The code that erases a disk is only in eclipse-flash.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod fonts;
mod rights;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "Usage: eclipse-flash-app [--screenshot <png>] [<image>]";

fn main() -> ExitCode {
    let mut start = ui::Start::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--screenshot" => match args.next() {
                Some(path) => start.screenshot = Some(PathBuf::from(path)),
                None => return usage("--screenshot needs a file"),
            },
            flag if flag.starts_with('-') => {
                return usage(&format!("unknown argument `{flag}`"));
            }
            _ if start.image.is_none() => start.image = Some(PathBuf::from(&arg)),
            _ => return usage("the app takes one image"),
        }
    }
    match ui::run(start) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("eclipse-flash-app: {why}");
            ExitCode::FAILURE
        }
    }
}

fn usage(why: &str) -> ExitCode {
    eprintln!("eclipse-flash-app: {why}\n{USAGE}");
    ExitCode::from(2)
}
