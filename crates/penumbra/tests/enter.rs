//! `penumbra enter` on the computer the tests run on, checked from outside: what it can read and
//! change, and the filters it runs under. The boot test does the same for `rift run --sandbox` in
//! the image, where bwrap builds the rest of the sandbox.
#![cfg(target_os = "linux")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// What the programs here read: their files and libraries, on this system or in a Nix build.
const SYSTEM: &[&str] = &[
    "/nix/store",
    "/usr",
    "/bin",
    "/lib",
    "/lib64",
    "/etc",
    "/proc",
];

fn folder(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("penumbra-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

/// Runs a command under `penumbra enter`, or `None` on a kernel older than the Landlock penumbra
/// needs, which the test then says and skips.
fn enter(read: &[&Path], write: &[&Path], command: &[&str]) -> Option<Output> {
    let mut enter = Command::new(env!("CARGO_BIN_EXE_penumbra"));
    enter.arg("enter");
    for path in SYSTEM.iter().map(Path::new).chain(read.iter().copied()) {
        enter.arg("--read").arg(path);
    }
    for path in write {
        enter.arg("--write").arg(path);
    }
    let output = enter.arg("--").args(command).output().unwrap();
    let said = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(126) && said.contains("does not enforce") {
        eprintln!("skipped: {said}");
        return None;
    }
    Some(output)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn changes_only_what_it_was_given() {
    let (inside, outside) = (folder("inside"), folder("outside"));
    let script = format!(
        "echo made > {}/made.txt; echo made > {}/made.txt",
        inside.display(),
        outside.display()
    );
    let Some(output) = enter(&[], &[&inside], &["sh", "-c", &script]) else {
        return;
    };
    assert!(inside.join("made.txt").exists(), "{}", text(&output.stderr));
    assert!(!outside.join("made.txt").exists());
    assert!(text(&output.stderr).contains("Permission denied"));
}

#[test]
fn reads_only_what_it_was_given() {
    let (shown, hidden) = (folder("shown"), folder("hidden"));
    fs::write(shown.join("a.txt"), "shown 3921\n").unwrap();
    fs::write(hidden.join("b.txt"), "hidden 3921\n").unwrap();
    let script = format!(
        "cat {0}/a.txt; cat {1}/b.txt; echo changed > {0}/a.txt",
        shown.display(),
        hidden.display()
    );
    let Some(output) = enter(&[&shown], &[], &["sh", "-c", &script]) else {
        return;
    };
    let printed = text(&output.stdout);
    assert!(printed.contains("shown 3921"), "{printed}");
    assert!(!printed.contains("hidden 3921"), "{printed}");
    assert_eq!(
        fs::read_to_string(shown.join("a.txt")).unwrap(),
        "shown 3921\n"
    );
}

#[test]
fn no_new_privileges_a_seccomp_filter_and_no_writes_to_proc() {
    let script = "grep -E '^(NoNewPrivs|Seccomp):' /proc/self/status; \
                  echo renamed > /proc/self/comm && echo written";
    let Some(output) = enter(&[], &[], &["sh", "-c", script]) else {
        return;
    };
    let printed = text(&output.stdout);
    assert!(printed.contains("NoNewPrivs:\t1"), "{printed}");
    assert!(printed.contains("Seccomp:\t2"), "{printed}");
    assert!(!printed.contains("written"), "{printed}");
}

#[test]
fn a_command_that_is_not_there() {
    let Some(output) = enter(&[], &[], &["no-such-program-3921"]) else {
        return;
    };
    assert_eq!(output.status.code(), Some(127));
    assert_eq!(
        text(&output.stderr).trim(),
        "no-such-program-3921 was not found in the sandbox."
    );
}
