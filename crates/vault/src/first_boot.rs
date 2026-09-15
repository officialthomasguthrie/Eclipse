//! vault-first-boot: runs in the initrd of every boot, before persist is opened. A drive written on
//! macOS or Windows, or with `rift-flash write --first-boot`, has no persist yet. On such a drive
//! this asks for a passphrase on the drive's own screen, makes persist in the free space after the
//! last partition the way rift-flash and a clone make it, and leaves it open for the boot to go on
//! with, so the passphrase is asked for once. It formats an exchange partition that has no file
//! system too. On a drive that has both it changes nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use librift::disk::run::{Persist, feed, settle, tool};
use librift::disk::{
    ALIGN, LEAST_PERSIST, LINUX_TYPE, Table, passphrase_problem, read_table, size,
};

/// The variable systemd-boot leaves with the partition uuid of the esp it was started from.
const LOADER_PARTITION: &str =
    "/sys/firmware/efi/efivars/LoaderDevicePartUUID-4a67b082-0a4c-41cf-b6c7-440b29bb8c4f";
/// Where udev names partitions by their uuids.
const PARTITIONS: &str = "/dev/disk/by-partuuid";
/// Where persist is mounted while it is filled.
const RUN: &str = "/run/vault-first-boot";
/// The label of persist, and the name it opens as, which the initrd mounts it from.
const PERSIST: &str = "persist";
/// How long the esp gets to show up.
const WAIT: Duration = Duration::from_secs(60);
/// The sectors at the end of a disk that hold the backup of its partition table.
const BACKUP: u64 = 33;

/// The two questions, and what is said before the first one again when an answer will not do.
const CHOOSE: &str = "Choose a passphrase";
const AGAIN: &str = "Type the passphrase again";
const DIFFERENT: &str = "The two passphrases are not the same.";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let disk = boot_disk()?;
    let table = read_disk(&disk)?;
    if let Some(exchange) = table.named("exchange") {
        if !holds_something(&exchange.node)? {
            println!("Formatting the exchange partition.");
            tool(
                Command::new("mkfs.exfat")
                    .args(["-L", "EXCHANGE"])
                    .arg(&exchange.node),
            )?;
        }
    }
    // a partition labelled persist with nothing on it is one a first boot added and did not format
    let persist = table.named(PERSIST);
    match persist {
        Some(partition) if holds_something(&partition.node)? => return Ok(()),
        Some(_) => {}
        None => {
            let printed = tool(Command::new("blockdev").arg("--getsize64").arg(&disk))?;
            let bytes = printed
                .trim()
                .parse::<u64>()
                .map_err(|e| format!("blockdev printed no size for {disk}: {e}"))?;
            let free = free_after(&table, bytes);
            if free < LEAST_PERSIST {
                return Err(format!(
                    "{disk} has {} free after its last partition, and persist needs {}.",
                    size(free),
                    size(LEAST_PERSIST)
                ));
            }
        }
    }
    let passphrase = choose(&mut ask)?;
    println!("Making persist.");
    let node = match persist {
        Some(partition) => partition.node.clone(),
        None => append(&disk)?,
    };
    let made = Persist::make(
        Path::new(&node),
        &passphrase,
        PERSIST,
        Path::new(RUN).join(PERSIST),
    )?;
    made.fill()?;
    made.leave_open()?;
    println!("Made persist on {node}.");
    Ok(())
}

/// The disk this system started from: the one that holds the esp systemd-boot was started from.
fn boot_disk() -> Result<String, String> {
    let variable = fs::read(LOADER_PARTITION)
        .map_err(|e| format!("Could not read which partition systemd-boot started from: {e}"))?;
    let uuid = loader_partition(&variable)
        .ok_or("systemd-boot left a partition uuid that cannot be read.")?;
    let partition = Path::new(PARTITIONS).join(&uuid);
    let start = Instant::now();
    while !partition.exists() {
        if start.elapsed() > WAIT {
            return Err(format!(
                "The partition systemd-boot started from, {uuid}, did not show up."
            ));
        }
        let _ = Command::new("udevadm")
            .args(["settle", "--timeout", "5"])
            .output();
        thread::sleep(Duration::from_millis(200));
    }
    let parent = tool(
        Command::new("lsblk")
            .args(["--noheadings", "--nodeps", "--output", "PKNAME"])
            .arg(&partition),
    )?;
    match parent.trim() {
        "" => Err(format!("{} is not on a disk.", partition.display())),
        name => Ok(format!("/dev/{name}")),
    }
}

/// The partition uuid in `LoaderDevicePartUUID`: four bytes of attributes, then the uuid in UTF-16
/// ending in a zero. In lower case, the way udev names partitions.
fn loader_partition(variable: &[u8]) -> Option<String> {
    let units: Vec<u16> = variable
        .get(4..)?
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|&unit| unit != 0)
        .collect();
    let uuid = String::from_utf16(&units).ok()?.to_ascii_lowercase();
    (uuid.len() == 36 && uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-')).then_some(uuid)
}

fn read_disk(disk: &str) -> Result<Table, String> {
    read_table(&tool(Command::new("sfdisk").arg("--json").arg(disk))?)
}

/// Whether blkid finds a file system, a LUKS header or anything else it knows on a partition.
fn holds_something(node: &str) -> Result<bool, String> {
    // without --no-part-details a probe prints the partition's own table entry and finds something
    // on every partition
    let probed = Command::new("blkid")
        .args(["--probe", "--no-part-details", node])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Could not run blkid: {e}"))?;
    // 2 is nothing found, 8 is more than one thing found
    match probed.status.code() {
        Some(0 | 8) => Ok(true),
        Some(2) => Ok(false),
        _ => Err(format!(
            "Could not look at {node}: {}",
            String::from_utf8_lossy(&probed.stderr).trim()
        )),
    }
}

/// The bytes a partition added after the last one gets: from the next MiB on to where the backup of
/// the table starts.
fn free_after(table: &Table, bytes: u64) -> u64 {
    let end = table
        .partitions
        .iter()
        .map(|partition| partition.start + partition.size)
        .max()
        .unwrap_or(0);
    let start = end.div_ceil(ALIGN) * ALIGN;
    let usable = (bytes / table.sectorsize).saturating_sub(BACKUP);
    usable.saturating_sub(start) * table.sectorsize
}

/// Adds a partition labelled persist in the free space after the last partition, and tells the
/// kernel about that one alone, since the others are in use.
fn append(disk: &str) -> Result<String, String> {
    feed(
        Command::new("sfdisk")
            .args([
                "--append",
                "--no-reread",
                "--no-tell-kernel",
                "--wipe-partitions",
                "always",
                "--quiet",
            ])
            .arg(disk),
        &format!("type={LINUX_TYPE}, name=\"{PERSIST}\"\n"),
    )?;
    let table = read_disk(disk)?;
    let persist = table
        .named(PERSIST)
        .ok_or_else(|| format!("sfdisk added no partition labelled persist to {disk}."))?;
    let number = number(&persist.node)
        .ok_or_else(|| format!("sfdisk named the new partition {}.", persist.node))?;
    // udev may have had the kernel read the whole table when sfdisk closed the disk. update adds the
    // partition when the kernel does not know it and leaves it as it is when it does
    tool(
        Command::new("partx")
            .args(["--update", "--nr", number])
            .arg(disk),
    )?;
    settle(&[PathBuf::from(&persist.node)])?;
    Ok(persist.node.clone())
}

/// The number at the end of a partition's device, 7 for /dev/nvme0n1p7.
fn number(node: &str) -> Option<&str> {
    let digits = node.len() - node.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    (digits > 0).then(|| &node[node.len() - digits..])
}

/// Asks until a passphrase has at least 8 characters and is typed the same twice.
fn choose(ask: &mut impl FnMut(&str) -> Result<String, String>) -> Result<String, String> {
    let mut question = CHOOSE.to_string();
    loop {
        let passphrase = ask(&question)?;
        if let Some(problem) = passphrase_problem(&passphrase) {
            question = format!("{problem} {CHOOSE}");
            continue;
        }
        if ask(AGAIN)? == passphrase {
            return Ok(passphrase);
        }
        question = format!("{DIFFERENT} {CHOOSE}");
    }
}

/// Asks through systemd's password agents: the splash on a screen, and a console that runs one.
fn ask(question: &str) -> Result<String, String> {
    let asked = Command::new("systemd-ask-password")
        .args([
            "--no-tty",
            "--timeout=0",
            "--icon=drive-harddisk",
            "--id=vault-first-boot:persist",
        ])
        .arg(question)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Could not ask for a passphrase: {e}"))?;
    if !asked.status.success() {
        return Err(format!(
            "Could not ask for a passphrase: {}",
            String::from_utf8_lossy(&asked.stderr).trim()
        ));
    }
    let mut answer =
        String::from_utf8(asked.stdout).map_err(|_| "The passphrase is not text.".to_string())?;
    if answer.ends_with('\n') {
        answer.pop();
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_boot_names_the_esp_in_utf_16() {
        let mut variable = vec![6, 0, 0, 0];
        for unit in "0B1C2D3E-4F50-4162-8374-859607A8B9CA"
            .encode_utf16()
            .chain([0])
        {
            variable.extend(unit.to_le_bytes());
        }
        assert_eq!(
            loader_partition(&variable).as_deref(),
            Some("0b1c2d3e-4f50-4162-8374-859607a8b9ca")
        );
        assert_eq!(loader_partition(&variable[..4]), None);
        assert_eq!(loader_partition(&variable[..3]), None);
        assert_eq!(loader_partition(&[6, 0, 0, 0, b'x', 0, 0, 0]), None);
    }

    #[test]
    fn persist_gets_the_space_sfdisk_gives_it() {
        // what rift-flash writes onto 24G on macOS and Windows, with an exchange partition. sfdisk
        // gave persist 8386527 sectors after these on the Linux writer's drive
        let table = read_table(
            r#"{"partitiontable": {"label": "gpt", "sectorsize": 512, "partitions": [
                {"node": "/dev/nvme0n1p1", "start": 2048, "size": 2097152, "type": "C12A7328-F81F-11D2-BA4B-00A0C93EC93B", "name": "esp"},
                {"node": "/dev/nvme0n1p2", "start": 2099200, "size": 2097152, "type": "77FF5F63-E7B6-4633-ACF4-1565B864C0E6", "name": "store-verity_0.1.0"},
                {"node": "/dev/nvme0n1p3", "start": 4196352, "size": 16777216, "type": "8484680C-9521-48C6-9C11-B0720656F69E", "name": "store_0.1.0"},
                {"node": "/dev/nvme0n1p4", "start": 20973568, "size": 2097152, "type": "77FF5F63-E7B6-4633-ACF4-1565B864C0E6", "name": "_empty"},
                {"node": "/dev/nvme0n1p5", "start": 23070720, "size": 16777216, "type": "8484680C-9521-48C6-9C11-B0720656F69E", "name": "_empty"},
                {"node": "/dev/nvme0n1p6", "start": 39847936, "size": 2097152, "type": "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7", "name": "exchange"}
            ]}}"#,
        )
        .unwrap();
        assert_eq!(free_after(&table, 24 << 30), 8_386_527 * 512);
        assert!(table.named(PERSIST).is_none());
        // a disk that ends before the slots do has nothing free
        assert_eq!(free_after(&table, 16 << 30), 0);
    }

    #[test]
    fn partitions_are_numbered_at_the_end() {
        assert_eq!(number("/dev/nvme0n1p7"), Some("7"));
        assert_eq!(number("/dev/sdb12"), Some("12"));
        assert_eq!(number("/dev/sdb"), None);
    }

    #[test]
    fn a_passphrase_is_asked_for_again_until_it_will_do() {
        let mut answers = ["short", "rift-test", "rift-tset", "rift-test", "rift-test"].into_iter();
        let mut asked = Vec::new();
        let chosen = choose(&mut |question: &str| {
            asked.push(question.to_string());
            Ok(answers.next().unwrap().to_string())
        });
        assert_eq!(chosen, Ok("rift-test".to_string()));
        assert_eq!(
            asked,
            [
                "Choose a passphrase",
                "A passphrase needs at least 8 characters. Choose a passphrase",
                "Type the passphrase again",
                "The two passphrases are not the same. Choose a passphrase",
                "Type the passphrase again",
            ]
        );
        assert_eq!(
            choose(&mut |_: &str| Err("No agent answered.".to_string())),
            Err("No agent answered.".to_string())
        );
    }
}
