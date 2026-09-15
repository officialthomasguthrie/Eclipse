//! Starts `rift-flash write` the way the app does, through the system's prompt for the rights to
//! erase a disk, and prints what it says. CI runs it on macOS as root and on Windows as an
//! administrator, where the prompt asks nothing.
//!
//! `elevated <rift-flash> <image> <disk> <serial>`

#[path = "../src/rights.rs"]
mod rights;

use std::path::PathBuf;
use std::process::ExitCode;

use iced::futures::StreamExt;
use iced::futures::channel::mpsc;
use iced::futures::executor::block_on;

use rights::{Event, Job};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [program, image, disk, serial] = args.as_slice() else {
        eprintln!("Usage: elevated <rift-flash> <image> <disk> <serial>");
        return ExitCode::from(2);
    };
    let job = Job {
        program: PathBuf::from(program),
        image: PathBuf::from(image),
        disk: disk.clone(),
        serial: serial.clone(),
        passphrase: None,
    };
    let (sender, mut receiver) = mpsc::unbounded();
    rights::start(job, sender);
    while let Some(event) = block_on(receiver.next()) {
        match event {
            Event::Said(line) => println!("{line}"),
            Event::Failed(line) => eprintln!("{line}"),
            Event::Ended(true) => return ExitCode::SUCCESS,
            Event::Ended(false) => return ExitCode::FAILURE,
        }
    }
    ExitCode::FAILURE
}
