//! eclipse: the CLI. Talks to the same D-Bus services corona uses. Only the help text exists so far.

use std::process::ExitCode;

/// Subcommands and the phase in which each becomes real.
const COMMANDS: &[(&str, &str, &str)] = &[
    (
        "update",
        "Download the next system image into the inactive slot",
        "Phase 2",
    ),
    ("rollback", "Boot the previous system slot", "Phase 2"),
    ("snapshot", "Take or list Timeline snapshots", "Phase 2"),
    (
        "backup",
        "Run an encrypted backup to a Vault target",
        "Phase 2",
    ),
    (
        "clone",
        "Write a complete second drive with a fresh key",
        "Phase 2",
    ),
    (
        "host",
        "Show or edit what Syzygy remembers about this machine",
        "Phase 1",
    ),
    ("ai", "Talk to Aura from the terminal", "Phase 1"),
    ("run", "Run a command inside a Penumbra sandbox", "Phase 2"),
    (
        "doctor",
        "Diagnose the drive, the host, and the session",
        "Phase 1",
    ),
    ("flash", "Write Eclipse onto a drive", "Phase 2"),
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("--help" | "-h" | "help") => {
            usage();
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!("eclipse {}", libeclipse::VERSION);
            ExitCode::SUCCESS
        }
        Some(cmd) => {
            if let Some((name, what, phase)) = COMMANDS.iter().find(|(name, _, _)| *name == cmd) {
                eprintln!("eclipse {name}: {what}. Not implemented yet ({phase}).");
            } else {
                eprintln!("eclipse: unknown command `{cmd}`\n");
                usage();
            }
            ExitCode::from(2)
        }
    }
}

fn usage() {
    println!(
        "eclipse {}\nYour computer, in your pocket.\n",
        libeclipse::VERSION
    );
    println!("Usage: eclipse <command> [args]\n\nCommands:");
    for (name, what, _) in COMMANDS {
        println!("  {name:<10} {what}");
    }
}
