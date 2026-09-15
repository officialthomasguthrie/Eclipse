//! eclipse: the CLI. `host`, `ai`, `doctor`, `snapshot`, `backup` and `net` ask the same D-Bus
//! services Corona uses, `clone` runs Vault as root, and `run --sandbox` runs Penumbra. The other
//! commands are only a line in the help so far.

mod ai;
mod backup;
mod clone;
mod doctor;
mod host;
mod net;
mod restore;
mod run;
mod search;
mod snapshot;
mod text;
mod version;

use std::process::ExitCode;

/// Subcommands and the phase in which each becomes real.
const COMMANDS: &[(&str, &str, &str)] = &[
    (
        "update",
        "Download the next system image into the inactive slot",
        "Phase 2",
    ),
    ("rollback", "Boot the previous system slot", "Phase 2"),
    (
        "snapshot",
        "List, take and restore from Timeline snapshots",
        "Phase 2",
    ),
    (
        "backup",
        "List, make and restore from encrypted backups",
        "Phase 2",
    ),
    (
        "clone",
        "Write a complete second drive with a fresh key",
        "Phase 2",
    ),
    (
        "host",
        "Show what Syzygy remembers about this machine",
        "Phase 1",
    ),
    (
        "ai",
        "Ask Aura a question, or search home by meaning",
        "Phase 1",
    ),
    ("run", "Run a command inside a Penumbra sandbox", "Phase 2"),
    (
        "net",
        "Turn the network off or on for an app in a sandbox",
        "Phase 2",
    ),
    (
        "doctor",
        "Check the drive, the host, and the services",
        "Phase 1",
    ),
    ("flash", "Write Eclipse onto a drive", "Phase 2"),
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        None | Some("--help" | "-h" | "help") => {
            usage();
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => version::run(rest),
        Some("host") => host::run(rest),
        Some("ai") => ai::run(rest),
        Some("doctor") => doctor::run(rest),
        Some("snapshot") => snapshot::run(rest),
        Some("backup") => backup::run(rest),
        Some("clone") => clone::run(rest),
        Some("run") => run::run(rest),
        Some("net") => net::run(rest),
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
