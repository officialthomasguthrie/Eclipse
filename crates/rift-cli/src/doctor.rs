//! `rift doctor`: one row per check with the numbers it read, then a count. It exits with 1
//! when a check failed. A warning is a number worth a look, not a fault, and leaves the exit code
//! alone.

use std::fmt::Write as _;
use std::fs;
use std::process::{Command, ExitCode};

use librift::aura::{self, Status};
use librift::{paths, syzygy};

use crate::text;

const USAGE: &str = "Usage: rift doctor";

/// Persist fails with less free space than the first number, in percent, and warns under the
/// second.
const PERSIST_FREE: (u64, u64) = (2, 10);
/// Memory fails with less available than the first number, in percent of the total, and warns
/// under the second.
const MEMORY_AVAILABLE: (u64, u64) = (5, 15);
/// A resource warns when some task waited for it this many percent of the last minute or more.
const MEMORY_STALL: f64 = 10.0;
const CPU_STALL: f64 = 50.0;
const IO_STALL: f64 = 25.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Passed,
    Warning,
    Failed,
}

impl Verdict {
    fn word(self) -> &'static str {
        match self {
            Self::Passed => "Passed",
            Self::Warning => "Warning",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Check {
    name: &'static str,
    verdict: Verdict,
    detail: String,
}

impl Check {
    fn new(name: &'static str, (verdict, detail): (Verdict, String)) -> Self {
        Self {
            name,
            verdict,
            detail,
        }
    }
}

pub fn run(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        None => {}
        Some("--help" | "-h") => {
            println!(
                "{USAGE}\n\nChecks the services, the drive and the machine, one row per check. \
                 Exits with 1 when a check failed."
            );
            return ExitCode::SUCCESS;
        }
        Some(other) => return text::unknown("doctor", other, USAGE),
    }
    let mountinfo = read("/proc/self/mountinfo");
    let checks = [
        Check::new("Syzygy", syzygy_verdict()),
        Check::new(
            "Aura",
            aura::status()
                .map_or_else(|why| (Verdict::Failed, why), |status| aura_verdict(&status)),
        ),
        Check::new(
            "Persist",
            persist_verdict(
                mount(&mountinfo, paths::PERSIST).as_ref(),
                space(paths::PERSIST),
            ),
        ),
        Check::new(
            "Memory",
            memory_verdict(
                &read("/proc/meminfo"),
                stall(&read("/proc/pressure/memory")),
            ),
        ),
        Check::new(
            "CPU",
            pressure_verdict("the CPU", stall(&read("/proc/pressure/cpu")), CPU_STALL),
        ),
        Check::new(
            "IO",
            pressure_verdict("IO", stall(&read("/proc/pressure/io")), IO_STALL),
        ),
        Check::new("System image", image_verdict(&mountinfo)),
    ];
    print!("{}", report(&checks));
    if checks.iter().any(|check| check.verdict == Verdict::Failed) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// A file under /proc or /sys, empty when it cannot be read.
fn read(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn report(checks: &[Check]) -> String {
    let width = checks
        .iter()
        .map(|check| check.name.chars().count() + 2)
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for check in checks {
        let _ = writeln!(
            out,
            "{:<width$}{:<9}{}",
            check.name,
            check.verdict.word(),
            check.detail.trim_end_matches('.')
        );
    }
    let count = |verdict: Verdict| checks.iter().filter(|c| c.verdict == verdict).count();
    let warnings = count(Verdict::Warning);
    let _ = writeln!(
        out,
        "\n{} checks, {} passed, {warnings} {}, {} failed.",
        checks.len(),
        count(Verdict::Passed),
        if warnings == 1 { "warning" } else { "warnings" },
        count(Verdict::Failed)
    );
    out
}

fn syzygy_verdict() -> (Verdict, String) {
    match syzygy::host() {
        Ok(host) => (
            Verdict::Passed,
            format!(
                "On the bus, host {}, class {}, AI tier {}",
                text::short(&host.fingerprint),
                host.class,
                host.ai_tier
            ),
        ),
        Err(why) => (Verdict::Failed, why),
    }
}

fn aura_verdict(status: &Status) -> (Verdict, String) {
    let model = if status.tier.is_empty() {
        status.model.clone()
    } else {
        format!("{} for tier {}", status.model, status.tier)
    };
    let error = |otherwise: &str| {
        if status.error.is_empty() {
            otherwise.to_string()
        } else {
            status.error.clone()
        }
    };
    match status.state.as_str() {
        "ready" => (Verdict::Passed, format!("Ready, {model}")),
        "loading" => (Verdict::Warning, format!("Loading {model}")),
        "none" => (Verdict::Warning, error("No chat model is on the drive")),
        "failed" => (Verdict::Failed, error("The model stopped")),
        other => (
            Verdict::Failed,
            format!("Aura says its state is {other}, which this program does not know"),
        ),
    }
}

/// The parts of a line of /proc/self/mountinfo the checks use.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Mount {
    /// `major:minor` of the device.
    device: String,
    fstype: String,
    source: String,
}

/// What is mounted at `point`. When there are several, the last one is on top.
fn mount(mountinfo: &str, point: &str) -> Option<Mount> {
    mountinfo.lines().rev().find_map(|line| {
        let (fields, rest) = line.split_once(" - ")?;
        let fields: Vec<&str> = fields.split(' ').collect();
        if fields.get(4).copied() != Some(point) {
            return None;
        }
        let mut rest = rest.split(' ');
        let fstype = rest.next()?;
        let source = rest.next()?;
        Some(Mount {
            device: (*fields.get(2)?).to_string(),
            fstype: fstype.to_string(),
            source: source.to_string(),
        })
    })
}

/// Free and total bytes of the file system under `path`.
fn space(path: &str) -> Option<(u64, u64)> {
    let stat = rustix::fs::statvfs(path).ok()?;
    Some((
        stat.f_bavail.saturating_mul(stat.f_frsize),
        stat.f_blocks.saturating_mul(stat.f_frsize),
    ))
}

fn persist_verdict(mount: Option<&Mount>, space: Option<(u64, u64)>) -> (Verdict, String) {
    let Some(mount) = mount else {
        return (
            Verdict::Failed,
            format!("Nothing is mounted at {}", paths::PERSIST),
        );
    };
    let place = format!("{} on {} ({})", paths::PERSIST, mount.source, mount.fstype);
    let Some((free, total)) = space.filter(|(_, total)| *total > 0) else {
        return (
            Verdict::Failed,
            format!("{place}, its free space could not be read"),
        );
    };
    let percent = free.saturating_mul(100) / total;
    let verdict = if percent < PERSIST_FREE.0 {
        Verdict::Failed
    } else if percent < PERSIST_FREE.1 {
        Verdict::Warning
    } else {
        Verdict::Passed
    };
    (
        verdict,
        format!(
            "{place}, {} free of {}, {percent} percent",
            text::size(free),
            text::size(total)
        ),
    )
}

/// A line of /proc/meminfo in bytes.
fn meminfo(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let value = line.strip_prefix(key)?.strip_prefix(':')?.trim();
        let kib: u64 = value.strip_suffix("kB")?.trim().parse().ok()?;
        Some(kib.saturating_mul(1024))
    })
}

/// From a file under /proc/pressure: the percent of the last minute in which some task waited.
fn stall(pressure: &str) -> Option<f64> {
    pressure.lines().find_map(|line| {
        line.strip_prefix("some ")?
            .split_whitespace()
            .find_map(|field| field.strip_prefix("avg60="))?
            .parse()
            .ok()
    })
}

fn memory_verdict(meminfo_text: &str, stalled: Option<f64>) -> (Verdict, String) {
    let (Some(total), Some(available)) = (
        meminfo(meminfo_text, "MemTotal"),
        meminfo(meminfo_text, "MemAvailable"),
    ) else {
        return (
            Verdict::Failed,
            "The memory figures in /proc/meminfo could not be read".into(),
        );
    };
    let percent = available.saturating_mul(100) / total.max(1);
    let mut verdict = if percent < MEMORY_AVAILABLE.0 {
        Verdict::Failed
    } else if percent < MEMORY_AVAILABLE.1 {
        Verdict::Warning
    } else {
        Verdict::Passed
    };
    let mut detail = format!(
        "{} available of {}, {percent} percent",
        text::size(available),
        text::size(total)
    );
    match stalled {
        Some(stalled) => {
            let _ = write!(
                detail,
                ", tasks waited for memory {stalled:.2} percent of the last 60 s"
            );
            if stalled >= MEMORY_STALL && verdict == Verdict::Passed {
                verdict = Verdict::Warning;
            }
        }
        None => detail.push_str(", no pressure figures in this kernel"),
    }
    (verdict, detail)
}

fn pressure_verdict(what: &str, stalled: Option<f64>, limit: f64) -> (Verdict, String) {
    match stalled {
        Some(stalled) => (
            if stalled >= limit {
                Verdict::Warning
            } else {
                Verdict::Passed
            },
            format!("Tasks waited for {what} {stalled:.2} percent of the last 60 s"),
        ),
        None => (
            Verdict::Warning,
            "No pressure figures in this kernel".into(),
        ),
    }
}

/// The store is the read-only system image under /usr, checked block by block by dm-verity.
fn image_verdict(mountinfo: &str) -> (Verdict, String) {
    let Some(usr) = mount(mountinfo, "/usr") else {
        return (Verdict::Failed, "Nothing is mounted at /usr".into());
    };
    let dm = format!("/sys/dev/block/{}/dm", usr.device);
    let name = read(&format!("{dm}/name"));
    let name = name.trim();
    let cmdline = read("/proc/cmdline");
    verity_verdict(
        &usr,
        read(&format!("{dm}/uuid")).trim(),
        name,
        root_hash(&cmdline),
        veritysetup(name).as_deref(),
    )
}

fn verity_verdict(
    usr: &Mount,
    uuid: &str,
    name: &str,
    hash: Option<&str>,
    status: Option<&str>,
) -> (Verdict, String) {
    if !uuid.starts_with("CRYPT-VERITY-") {
        return (
            Verdict::Failed,
            format!(
                "/usr is {} ({}), not a verity device",
                usr.source, usr.fstype
            ),
        );
    }
    let mut verdict = Verdict::Passed;
    let mut detail = format!("/usr on verity device {name}");
    if let Some(hash) = hash {
        let _ = write!(detail, ", root hash {}", text::short(hash));
    } else {
        detail.push_str(", no root hash on the kernel command line");
        verdict = Verdict::Warning;
    }
    match status {
        None => {}
        Some("verified") => detail.push_str(", verified"),
        Some(other) => {
            let _ = write!(detail, ", veritysetup says {other}");
            verdict = Verdict::Failed;
        }
    }
    (verdict, detail)
}

fn root_hash(cmdline: &str) -> Option<&str> {
    cmdline
        .split_whitespace()
        .find_map(|word| word.strip_prefix("usrhash="))
}

/// What veritysetup says about the device, `verified` or `corrupted`. It needs root to read it,
/// so for anyone else this is nothing and the row goes without it.
fn veritysetup(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let output = Command::new("veritysetup")
        .args(["status", name])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("status:")
                .map(|s| s.trim().to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTINFO: &str = "\
25 1 0:22 / / rw,relatime shared:1 - tmpfs tmpfs rw,size=1002464k,mode=755
30 25 254:0 / /usr ro,relatime shared:2 - erofs /dev/mapper/usr ro,user_xattr,acl
31 25 254:0 /nix/store /nix/store ro,relatime shared:2 - erofs /dev/mapper/usr ro,user_xattr,acl
40 25 0:35 / /persist rw,noatime shared:10 - btrfs /dev/mapper/persist rw,compress=zstd:3,subvolid=5,subvol=/
";

    const MEMINFO: &str = "\
MemTotal:        4012348 kB
MemFree:          812044 kB
MemAvailable:    2206792 kB
Buffers:            2040 kB
";

    fn persist() -> Mount {
        mount(MOUNTINFO, "/persist").unwrap()
    }

    fn usr() -> Mount {
        mount(MOUNTINFO, "/usr").unwrap()
    }

    fn near(value: Option<f64>, expected: f64) -> bool {
        value.is_some_and(|value| (value - expected).abs() < 1e-9)
    }

    #[test]
    fn mounts_are_found_by_where_they_are() {
        assert_eq!(
            usr(),
            Mount {
                device: "254:0".into(),
                fstype: "erofs".into(),
                source: "/dev/mapper/usr".into(),
            }
        );
        assert_eq!(persist().source, "/dev/mapper/persist");
        assert_eq!(mount(MOUNTINFO, "/home"), None);
        assert_eq!(mount(MOUNTINFO, "/nix"), None);
        assert_eq!(mount("garbage\n\n", "/usr"), None);
    }

    #[test]
    fn the_mount_on_top_wins() {
        let shadowed = format!("{MOUNTINFO}50 40 0:40 / /persist rw shared:20 - tmpfs tmpfs rw\n");
        assert_eq!(mount(&shadowed, "/persist").unwrap().fstype, "tmpfs");
    }

    #[test]
    fn persist_says_how_much_is_free() {
        let gib = 1 << 30;
        assert_eq!(
            persist_verdict(Some(&persist()), Some((1_288_490_189, 2 * gib))),
            (
                Verdict::Passed,
                "/persist on /dev/mapper/persist (btrfs), 1.2 GiB free of 2.0 GiB, 60 percent"
                    .to_string()
            )
        );
        assert_eq!(
            persist_verdict(Some(&persist()), Some((gib / 8, 2 * gib))).0,
            Verdict::Warning
        );
        assert_eq!(
            persist_verdict(Some(&persist()), Some((gib / 64, 2 * gib))).0,
            Verdict::Failed
        );
        assert_eq!(persist_verdict(Some(&persist()), None).0, Verdict::Failed);
        assert_eq!(
            persist_verdict(None, Some((gib, gib))),
            (
                Verdict::Failed,
                "Nothing is mounted at /persist".to_string()
            )
        );
    }

    #[test]
    fn pressure_is_the_minute_average() {
        let cpu = "some avg10=3.10 avg60=1.25 avg300=0.40 total=123456\n\
                   full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n";
        assert!(near(stall(cpu), 1.25));
        assert_eq!(
            stall("full avg10=0.00 avg60=7.00 avg300=0.00 total=0\n"),
            None
        );
        assert_eq!(stall(""), None);
        assert_eq!(
            pressure_verdict("the CPU", Some(1.25), CPU_STALL),
            (
                Verdict::Passed,
                "Tasks waited for the CPU 1.25 percent of the last 60 s".to_string()
            )
        );
        assert_eq!(
            pressure_verdict("IO", Some(IO_STALL), IO_STALL).0,
            Verdict::Warning
        );
        assert_eq!(pressure_verdict("IO", None, IO_STALL).0, Verdict::Warning);
    }

    #[test]
    fn memory_says_how_much_is_available() {
        assert_eq!(meminfo(MEMINFO, "MemTotal"), Some(4_012_348 * 1024));
        assert_eq!(meminfo(MEMINFO, "Mem"), None);
        assert_eq!(
            memory_verdict(MEMINFO, Some(0.0)),
            (
                Verdict::Passed,
                "2.1 GiB available of 3.8 GiB, 55 percent, tasks waited for memory 0.00 percent \
                 of the last 60 s"
                    .to_string()
            )
        );
        assert_eq!(memory_verdict(MEMINFO, Some(12.5)).0, Verdict::Warning);
        let tight = "MemTotal: 4012348 kB\nMemAvailable: 120000 kB\n";
        assert_eq!(memory_verdict(tight, Some(0.0)).0, Verdict::Failed);
        assert!(
            memory_verdict(MEMINFO, None)
                .1
                .ends_with("no pressure figures in this kernel")
        );
        assert_eq!(memory_verdict("", None).0, Verdict::Failed);
    }

    #[test]
    fn the_image_has_to_be_a_verity_device() {
        let uuid = "CRYPT-VERITY-9f3c0a0b1c2d4e5f8a9b0c1d2e3f4a5b-usr";
        let hash = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d";
        assert_eq!(
            verity_verdict(&usr(), uuid, "usr", Some(hash), None),
            (
                Verdict::Passed,
                "/usr on verity device usr, root hash 1a2b3c4d5e6f".to_string()
            )
        );
        assert_eq!(
            verity_verdict(&usr(), uuid, "usr", Some(hash), Some("verified")).1,
            "/usr on verity device usr, root hash 1a2b3c4d5e6f, verified"
        );
        assert_eq!(
            verity_verdict(&usr(), uuid, "usr", Some(hash), Some("corrupted")).0,
            Verdict::Failed
        );
        assert_eq!(
            verity_verdict(&usr(), uuid, "usr", None, None).0,
            Verdict::Warning
        );
        assert_eq!(
            verity_verdict(&usr(), "", "", Some(hash), None),
            (
                Verdict::Failed,
                "/usr is /dev/mapper/usr (erofs), not a verity device".to_string()
            )
        );
        assert_eq!(
            root_hash("initrd=\\efi\\x init=/nix/store/abc/init usrhash=1a2b quiet"),
            Some("1a2b")
        );
        assert_eq!(root_hash("quiet"), None);
    }

    #[test]
    fn aura_is_ready_or_says_why_not() {
        let status = |state: &str, error: &str| Status {
            state: state.into(),
            model: "qwen3-0.6b-q8_0".into(),
            tier: "small".into(),
            error: error.into(),
            ..Status::default()
        };
        assert_eq!(
            aura_verdict(&status("ready", "")),
            (
                Verdict::Passed,
                "Ready, qwen3-0.6b-q8_0 for tier small".to_string()
            )
        );
        assert_eq!(aura_verdict(&status("loading", "")).0, Verdict::Warning);
        assert_eq!(
            aura_verdict(&status(
                "none",
                "No chat model that fits this machine is on the drive."
            )),
            (
                Verdict::Warning,
                "No chat model that fits this machine is on the drive.".to_string()
            )
        );
        assert_eq!(aura_verdict(&status("failed", "")).0, Verdict::Failed);
        assert_eq!(aura_verdict(&status("asleep", "")).0, Verdict::Failed);
    }

    #[test]
    fn the_report_is_rows_and_a_count() {
        let checks = [
            Check::new(
                "Syzygy",
                (
                    Verdict::Passed,
                    "On the bus, host 5297c0f65d6a, class borrowed, AI tier small".into(),
                ),
            ),
            Check::new(
                "Aura",
                (
                    Verdict::Warning,
                    "Loading qwen3-0.6b-q8_0 for tier small".into(),
                ),
            ),
            Check::new(
                "System image",
                (Verdict::Failed, "Nothing is mounted at /usr.".into()),
            ),
        ];
        assert_eq!(
            report(&checks),
            "Syzygy        Passed   On the bus, host 5297c0f65d6a, class borrowed, AI tier small\n\
             Aura          Warning  Loading qwen3-0.6b-q8_0 for tier small\n\
             System image  Failed   Nothing is mounted at /usr\n\
             \n\
             3 checks, 1 passed, 1 warning, 1 failed.\n"
        );
    }
}
