//! rift-flash, the command line. What it does is in the library, which the app uses too.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    rift_flash::cli(&args)
}
