//! Cloning: a second drive written onto a removable disk, that boots by itself and opens with a
//! passphrase of its own.
//!
//! The clone gets a new partition table. The running slot is copied as blocks into its slot A, its
//! slot B is empty, and its esp is new, with systemd-boot and the running version's uki. Persist is
//! new too: LUKS2 with a volume key of its own around a new btrfs, and every subvolume but the
//! snapshots is sent into it from a read-only snapshot of this drive's. Vault only writes onto a
//! whole disk that is removable or on USB, that nothing uses, and that the running system is not on.
//! The layout, the guard and the steps that write a drive are librift's, rift-flash uses them
//! too.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use librift::disk::run::{self, Mounted, Persist, copy, copy_file, feed, on_path, settle, tool};
pub use librift::disk::{Block, confirmation, describe, passphrase_problem};
use librift::disk::{
    LSBLK, MACHINE_ID, STORE_SIZE, Slot, Table, VERITY_SIZE, disks_in, machine_id, needed,
    partition_node, read_lsblk, read_table, refuse, script, size,
};

use crate::timeline;

/// The subvolumes a clone gets a copy of. `@snapshots` is made new and empty.
pub const SUBVOLUMES: [&str; 5] = ["@home", "@var", "@flatpak", "@models", "@hosts"];
/// Files in `@var` the system made for this drive alone. The clone makes its own.
const FORGET: [&str; 3] = [
    "lib/systemd/random-seed",
    "lib/systemd/credential.secret",
    "lib/NetworkManager/secret_key",
];
/// What the esp needs besides the uki.
const BOOT_FILES: [&str; 3] = [
    "EFI/BOOT/BOOTX64.EFI",
    "EFI/systemd/systemd-bootx64.efi",
    "loader/loader.conf",
];
/// The programs every clone runs, looked for before anything is erased.
const TOOLS: [&str; 11] = [
    "lsblk",
    "findmnt",
    "sfdisk",
    "udevadm",
    "veritysetup",
    "mkfs.vfat",
    "mount",
    "umount",
    "cryptsetup",
    "mkfs.btrfs",
    "btrfs",
];

/// The running slot in the running drive's table, from the partitions udev names `usr-verity` and
/// `usr`. Their labels have to carry the version that runs.
pub fn running_slot(
    table: &Table,
    verity: &str,
    store: &str,
    version: &str,
) -> Result<Slot, String> {
    let find = |node: &str, label: String| {
        let partition = table
            .partitions
            .iter()
            .find(|partition| partition.node == node)
            .ok_or_else(|| {
                format!("{node} is not in the partition table of the drive this system runs from.")
            })?;
        if partition.name.as_deref() != Some(label.as_str()) {
            return Err(format!(
                "{node} is labelled {}, not {label}, so it does not hold the version that runs.",
                partition.name.as_deref().unwrap_or("nothing")
            ));
        }
        if partition.uuid.is_none() {
            return Err(format!("{node} has no partition uuid."));
        }
        Ok(partition.clone())
    };
    Ok(Slot {
        version: version.to_string(),
        verity: find(verity, format!("store-verity_{version}"))?,
        store: find(store, format!("store_{version}"))?,
    })
}

/// The uki of `version` among the file names in the esp's `EFI/Linux`: `rift_0.2.0.efi`, or one
/// with a boot counter, `rift_0.2.0+2.efi` or `rift_0.2.0+1-2.efi`. The one without a counter
/// first.
pub fn uki(names: &[String], id: &str, version: &str) -> Option<String> {
    let prefix = format!("{id}_{version}");
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    let counter = |text: &str| match text.split_once('-') {
        Some((left, done)) => digits(left) && digits(done),
        None => digits(text),
    };
    let mut found: Vec<&String> = names
        .iter()
        .filter(|name| {
            name.strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(".efi"))
                .is_some_and(|rest| rest.is_empty() || rest.strip_prefix('+').is_some_and(counter))
        })
        .collect();
    found.sort_by_key(|name| (name.len(), name.as_str()));
    found.first().map(|name| (*name).clone())
}

/// `IMAGE_ID` and `IMAGE_VERSION` from the text of os-release, when both are plain words.
pub fn os_release(text: &str) -> Option<(String, String)> {
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(|value| value.trim().trim_matches(['"', '\'']).to_string())
            .filter(|value| {
                !value.is_empty()
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
    };
    Some((value("IMAGE_ID")?, value("IMAGE_VERSION")?))
}

/// The partitions a dm-verity device runs from, and how much of the data partition it maps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verity {
    pub data: PathBuf,
    pub hash: PathBuf,
    pub bytes: u64,
}

/// The data device, the hash device and the size in what `veritysetup status` printed. The size is
/// in sectors of 512 bytes, `12163072 [512-byte units] (6227492864 [bytes])` or `12163072 sectors`
/// in older versions, and nothing past it is ever read.
pub fn read_veritysetup(status: &str) -> Option<Verity> {
    let field = |name: &str| {
        status.lines().find_map(|line| {
            line.trim()
                .strip_prefix(name)?
                .strip_prefix(':')
                .map(str::trim)
        })
    };
    let sectors: u64 = field("size")?.split_whitespace().next()?.parse().ok()?;
    let data = field("data device")?;
    let hash = field("hash device")?;
    if sectors == 0 || !data.starts_with("/dev/") || !hash.starts_with("/dev/") {
        return None;
    }
    Some(Verity {
        data: PathBuf::from(data),
        hash: PathBuf::from(hash),
        bytes: sectors.checked_mul(512)?,
    })
}

pub fn send_args(snapshot: &Path) -> Vec<OsString> {
    vec!["send".into(), snapshot.into()]
}

pub fn receive_args(folder: &Path) -> Vec<OsString> {
    vec!["receive".into(), folder.into()]
}

/// Where a clone reads the running drive, and where it mounts what it writes.
#[derive(Debug, Clone)]
pub struct Cloner {
    /// The top of persist, `/persist`.
    pub persist: PathBuf,
    /// Where the snapshots a clone sends are taken, `/persist/@snapshots/clone`.
    pub snapshots: PathBuf,
    /// The running drive's esp, `/boot`.
    pub boot: PathBuf,
    /// udev's names for the running drive's partitions, `/dev/disk/by-designator`.
    pub designators: PathBuf,
    /// Where the clone's esp and persist are mounted while they are written, `/run/vault-clone`.
    pub run: PathBuf,
}

/// What a clone copies from the running drive.
#[derive(Debug, Clone)]
pub struct Running {
    /// The names of the disks the running system is on.
    pub disks: Vec<String>,
    pub slot: Slot,
    /// The size of the running drive's exchange partition, when it has one.
    pub exchange: Option<u64>,
    /// The running slot's partitions, and how much of each is copied.
    pub verity: PathBuf,
    pub verity_bytes: u64,
    pub store: PathBuf,
    pub data: u64,
    /// The running version's uki, and its name on the clone.
    pub uki: PathBuf,
    pub uki_name: String,
}

/// Everything a clone needs, checked before the disk is erased.
#[derive(Debug, Clone)]
pub struct Plan {
    pub disk: Block,
    pub running: Running,
}

impl Cloner {
    fn designator(&self, name: &str) -> Result<PathBuf, String> {
        fs::canonicalize(self.designators.join(name)).map_err(|_| {
            format!("udev does not name the {name} partition of the drive this system runs from.")
        })
    }

    /// The running drive: the disks it is on, the running slot, and what a clone copies from it.
    pub fn running(&self) -> Result<Running, String> {
        let esp = self.designator("esp")?;
        // the initrd names the device /usr runs from after it. its status has the two partitions of
        // the running slot and how much of the store dm-verity reads
        let status = tool(Command::new("veritysetup").args(["status", "usr"]))?;
        let Verity {
            data: store,
            hash: verity,
            bytes: data,
        } = read_veritysetup(&status)
            .ok_or("veritysetup does not say which partitions the system runs from.")?;
        let persist = tool(
            Command::new("findmnt")
                .args([
                    "--noheadings",
                    "--nofsroot",
                    "--output",
                    "SOURCE",
                    "--mountpoint",
                ])
                .arg(&self.persist),
        )?;
        let mut disks = Vec::new();
        for node in [&esp, &verity, &store, Path::new(persist.trim())] {
            disks.extend(disks_in(&tool(
                Command::new("lsblk")
                    .args([
                        "--inverse",
                        "--list",
                        "--noheadings",
                        "--output",
                        "NAME,TYPE",
                    ])
                    .arg(node),
            )?));
        }
        let boot = disks
            .first()
            .cloned()
            .ok_or_else(|| "Could not find the disk the esp of this system is on.".to_string())?;
        let table = read_table(&tool(
            Command::new("sfdisk")
                .arg("--json")
                .arg(Path::new("/dev").join(&boot)),
        )?)?;

        let release = fs::read_to_string("/etc/os-release")
            .map_err(|e| format!("Could not read /etc/os-release: {e}"))?;
        let (id, version) = os_release(&release)
            .ok_or("The running system does not say its image id and version.")?;
        let slot = running_slot(&table, &text(&verity)?, &text(&store)?, &version)?;
        let verity_bytes = table.bytes(&slot.verity);
        if verity_bytes > VERITY_SIZE || data > table.bytes(&slot.store) || data > STORE_SIZE {
            return Err(format!(
                "The system partitions of version {version} are bigger than a slot."
            ));
        }

        let linux = self.boot.join("EFI/Linux");
        let names: Vec<String> = fs::read_dir(&linux)
            .map_err(|e| format!("Could not read {}: {e}", linux.display()))?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        let found = uki(&names, &id, &version).ok_or_else(|| {
            format!(
                "There is no uki of version {version} in {}.",
                linux.display()
            )
        })?;
        if let Some(missing) = BOOT_FILES
            .iter()
            .map(|file| self.boot.join(file))
            .find(|path| !path.is_file())
        {
            return Err(format!("{} is missing from the esp.", missing.display()));
        }
        Ok(Running {
            disks,
            exchange: table
                .named("exchange")
                .map(|partition| table.bytes(partition)),
            slot,
            verity,
            verity_bytes,
            store,
            data,
            uki: linux.join(found),
            uki_name: format!("{id}_{version}.efi"),
        })
    }

    /// Looks at the running drive and at `disk`, and refuses the disk or says what the clone will be.
    pub fn inspect(&self, disk: &Path) -> Result<Plan, String> {
        let shown = disk.display();
        let path = fs::canonicalize(disk).map_err(|e| format!("There is no disk {shown}: {e}"))?;
        if let Some(missing) = TOOLS.iter().find(|name| !on_path(name)) {
            return Err(format!("{missing} is missing, and a clone needs it."));
        }
        let target = read_lsblk(&tool(
            Command::new("lsblk")
                .args(["--json", "--bytes", "--output", LSBLK])
                .arg(&path),
        )?)?;
        let running = self.running()?;
        if running.exchange.is_some() && !on_path("mkfs.exfat") {
            return Err("mkfs.exfat is missing, and the exchange partition needs it.".into());
        }
        refuse(
            &target,
            &running.disks,
            needed(used(&self.persist)?, running.exchange),
        )?;
        Ok(Plan {
            disk: target,
            running,
        })
    }

    /// Erases the plan's disk and writes the clone onto it, saying each step.
    pub fn write(
        &self,
        plan: &Plan,
        passphrase: &str,
        say: &mut impl FnMut(String),
    ) -> Result<(), String> {
        let _lock = timeline::lock(&self.snapshots)?;
        self.clear()?;
        let running = &plan.running;
        let disk = plan.disk.path.as_str();
        say(format!("Writing a new partition table on {disk}."));
        feed(
            Command::new("sfdisk").args(run::sfdisk_args(Path::new(disk))),
            &script(&running.slot, running.exchange),
        )?;
        let persist = if running.exchange.is_some() { 7 } else { 6 };
        let part = |number| PathBuf::from(partition_node(disk, number));
        settle(&(1..=persist).map(part).collect::<Vec<_>>())?;

        say("Writing the boot partition.".into());
        self.write_esp(running, &part(1))?;
        say(format!(
            "Copying the system, version {}, {}.",
            running.slot.version,
            size(running.data)
        ));
        copy(
            &mut open(&running.verity)?,
            &running.verity,
            &part(2),
            0,
            running.verity_bytes,
            false,
            &mut |_: String| {},
        )?;
        copy(
            &mut open(&running.store)?,
            &running.store,
            &part(3),
            0,
            running.data,
            false,
            say,
        )?;
        if running.exchange.is_some() {
            say("Formatting the exchange partition.".into());
            tool(
                Command::new("mkfs.exfat")
                    .args(["-L", "EXCHANGE"])
                    .arg(part(6)),
            )?;
        }
        self.write_persist(&part(persist), passphrase, say)
    }

    fn write_esp(&self, running: &Running, partition: &Path) -> Result<(), String> {
        tool(
            Command::new("mkfs.vfat")
                .args(["-F", "32", "-n", "ESP"])
                .arg(partition),
        )?;
        let esp = Mounted::new(
            partition,
            self.run.join(format!("esp-{}", std::process::id())),
            "vfat",
            None,
        )?;
        for file in BOOT_FILES {
            copy_file(&self.boot.join(file), &esp.path().join(file))?;
        }
        copy_file(
            &running.uki,
            &esp.path().join("EFI/Linux").join(&running.uki_name),
        )?;
        esp.unmount()
    }

    fn write_persist(
        &self,
        partition: &Path,
        passphrase: &str,
        say: &mut impl FnMut(String),
    ) -> Result<(), String> {
        say("Encrypting persist with a new key.".into());
        let pid = std::process::id();
        let persist = Persist::make(
            partition,
            passphrase,
            &format!("vault-clone-{pid}"),
            self.run.join(format!("persist-{pid}")),
        )?;
        self.fill(persist.top(), say)?;
        persist.close()
    }

    /// Sends each subvolume into the clone's persist at `top`, makes `@snapshots`, and gives the
    /// clone an identity of its own.
    fn fill(&self, top: &Path, say: &mut impl FnMut(String)) -> Result<(), String> {
        let received = top.join(".received");
        fs::create_dir(&received)
            .map_err(|e| format!("Could not make {}: {e}", received.display()))?;
        for subvolume in SUBVOLUMES {
            say(format!("Copying {}.", subvolume.trim_start_matches('@')));
            let snapshot = self.snapshots.join(subvolume);
            let source = self.persist.join(subvolume);
            timeline::btrfs([
                "subvolume".as_ref(),
                "snapshot".as_ref(),
                "-r".as_ref(),
                source.as_os_str(),
                snapshot.as_os_str(),
            ])?;
            let sent = send(&snapshot, &received);
            let deleted = delete(&snapshot);
            sent?;
            deleted?;
            // what btrfs receives is read only. a snapshot of it under the subvolume's own name is not
            let copied = received.join(subvolume);
            let writable = top.join(subvolume);
            timeline::btrfs([
                "subvolume".as_ref(),
                "snapshot".as_ref(),
                copied.as_os_str(),
                writable.as_os_str(),
            ])?;
            delete(&copied)?;
        }
        fs::remove_dir(&received)
            .map_err(|e| format!("Could not remove {}: {e}", received.display()))?;
        let snapshots = top.join("@snapshots");
        timeline::btrfs([
            "subvolume".as_ref(),
            "create".as_ref(),
            snapshots.as_os_str(),
        ])?;

        let var = top.join("@var");
        let id = var.join(MACHINE_ID);
        if let Some(parent) = id.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Could not make {}: {e}", parent.display()))?;
        }
        fs::write(&id, machine_id(run::random()?))
            .map_err(|e| format!("Could not write the clone's machine id: {e}"))?;
        for file in FORGET {
            match fs::remove_file(var.join(file)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => {
                    return Err(format!("Could not remove {file} from the clone: {e}"));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Deletes the snapshots a clone that did not finish left behind.
    fn clear(&self) -> Result<(), String> {
        for subvolume in SUBVOLUMES {
            let path = self.snapshots.join(subvolume);
            if path.exists() {
                delete(&path)?;
            }
        }
        Ok(())
    }
}

fn text(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(ToString::to_string)
        .ok_or_else(|| format!("{} is not a plain path.", path.display()))
}

fn open(path: &Path) -> Result<File, String> {
    File::open(path).map_err(|e| format!("Could not read {}: {e}", path.display()))
}

/// The bytes persist holds now.
fn used(persist: &Path) -> Result<u64, String> {
    let stat = rustix::fs::statvfs(persist)
        .map_err(|e| format!("Could not read how full {} is: {e}", persist.display()))?;
    Ok(stat.f_blocks.saturating_sub(stat.f_bfree) * stat.f_frsize)
}

/// Sends `snapshot` with btrfs send into `folder` with btrfs receive.
fn send(snapshot: &Path, folder: &Path) -> Result<(), String> {
    let mut sender = Command::new("btrfs")
        .args(send_args(snapshot))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not run btrfs send: {e}"))?;
    let stream = sender
        .stdout
        .take()
        .ok_or("btrfs send gave nothing to read.")?;
    let received = Command::new("btrfs")
        .args(receive_args(folder))
        .stdin(stream)
        .output()
        .map_err(|e| format!("Could not run btrfs receive: {e}"))?;
    let sent = sender
        .wait_with_output()
        .map_err(|e| format!("btrfs send stopped: {e}"))?;
    if !sent.status.success() {
        return Err(format!(
            "btrfs send {} failed: {}",
            snapshot.display(),
            String::from_utf8_lossy(&sent.stderr).trim()
        ));
    }
    if !received.status.success() {
        return Err(format!(
            "btrfs receive into {} failed: {}",
            folder.display(),
            String::from_utf8_lossy(&received.stderr).trim()
        ));
    }
    Ok(())
}

fn delete(subvolume: &Path) -> Result<(), String> {
    timeline::btrfs([
        "subvolume".as_ref(),
        "delete".as_ref(),
        subvolume.as_os_str(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The running drive after an update and a rollback: 0.2.0 runs from slot b, 0.3.0 is in slot a.
    const TABLE: &str = r#"{
       "partitiontable": {
          "label": "gpt", "id": "B581FEF7-24ED-4F31-990B-099EC86BBA03", "device": "/dev/nvme0n1",
          "unit": "sectors", "firstlba": 2048, "lastlba": 44171264, "sectorsize": 512,
          "partitions": [
             {"node": "/dev/nvme0n1p1", "start": 2048, "size": 2097152, "type": "C12A7328-F81F-11D2-BA4B-00A0C93EC93B",
              "uuid": "3A5A4E2B-1B1C-4F53-9D2E-0E4F1B2C3D4E", "name": "esp"},
             {"node": "/dev/nvme0n1p2", "start": 2099200, "size": 2097152, "type": "77FF5F63-E7B6-4633-ACF4-1565B864C0E6",
              "uuid": "06072C77-78C7-FA9E-7540-E489822F25FD", "name": "store-verity_0.3.0", "attrs": "GUID:60"},
             {"node": "/dev/nvme0n1p3", "start": 4196352, "size": 16777216, "type": "8484680C-9521-48C6-9C11-B0720656F69E",
              "uuid": "18285BF3-EA77-2240-3976-E8CD9FC1FBB6", "name": "store_0.3.0", "attrs": "GUID:60"},
             {"node": "/dev/nvme0n1p4", "start": 20973568, "size": 2097152, "type": "77FF5F63-E7B6-4633-ACF4-1565B864C0E6",
              "uuid": "5C1D8E0A-2B3C-4D5E-8F90-A1B2C3D4E5F6", "name": "store-verity_0.2.0", "attrs": "GUID:60"},
             {"node": "/dev/nvme0n1p5", "start": 23070720, "size": 16777216, "type": "8484680C-9521-48C6-9C11-B0720656F69E",
              "uuid": "7E8F9A0B-1C2D-4E3F-9051-627384950A1B", "name": "store_0.2.0"},
             {"node": "/dev/nvme0n1p6", "start": 39847936, "size": 4323328, "type": "0FC63DAF-8483-4772-8E79-3D69D8477DE4",
              "uuid": "9A0B1C2D-3E4F-4051-8263-748596A7B8C9", "name": "persist"}
          ]
       }
    }"#;

    #[test]
    fn the_running_slot_is_found_by_its_partitions_and_version() {
        let table = read_table(TABLE).unwrap();
        let slot = running_slot(&table, "/dev/nvme0n1p4", "/dev/nvme0n1p5", "0.2.0").unwrap();
        assert_eq!(slot.version, "0.2.0");
        assert_eq!(
            slot.verity.uuid.as_deref(),
            Some("5C1D8E0A-2B3C-4D5E-8F90-A1B2C3D4E5F6")
        );
        assert_eq!(slot.store.name.as_deref(), Some("store_0.2.0"));
        // slot a of the clone is this slot, under its uuids
        assert!(
            script(&slot, None)
                .lines()
                .nth(3)
                .is_some_and(|line| line.contains("uuid=7E8F9A0B-1C2D-4E3F-9051-627384950A1B"))
        );

        // os-release says 0.3.0 while 0.2.0's partitions run
        let why = running_slot(&table, "/dev/nvme0n1p4", "/dev/nvme0n1p5", "0.3.0").unwrap_err();
        assert!(
            why.contains("labelled store-verity_0.2.0, not store-verity_0.3.0"),
            "{why}"
        );
        assert!(running_slot(&table, "/dev/sda2", "/dev/sda3", "0.2.0").is_err());
    }

    #[test]
    fn the_running_uki_is_found_with_or_without_a_counter() {
        let names = |list: &[&str]| list.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            uki(
                &names(&["rift_0.2.0.efi", "rift_0.3.0+0-3.efi"]),
                "rift",
                "0.2.0"
            )
            .as_deref(),
            Some("rift_0.2.0.efi")
        );
        assert_eq!(
            uki(&names(&["rift_0.1.0+2-1.efi"]), "rift", "0.1.0").as_deref(),
            Some("rift_0.1.0+2-1.efi")
        );
        assert_eq!(
            uki(&names(&["rift_0.1.0+3.efi"]), "rift", "0.1.0").as_deref(),
            Some("rift_0.1.0+3.efi")
        );
        for other in [
            "rift_0.1.0.1.efi",
            "rift_0.1.0+.efi",
            "rift_0.1.0+a-1.efi",
            "rift_0.1.0+1-.efi",
            "rift_0.1.0.efi.bak",
            "other_0.1.0.efi",
        ] {
            assert_eq!(uki(&names(&[other]), "rift", "0.1.0"), None, "{other}");
        }
    }

    #[test]
    fn the_image_id_and_version_come_from_os_release() {
        let text = "NAME=NixOS\nIMAGE_ID=\"rift\"\nIMAGE_VERSION=\"0.2.0\"\nVERSION_ID=\"26.05\"\n";
        assert_eq!(
            os_release(text),
            Some(("rift".to_string(), "0.2.0".to_string()))
        );
        assert_eq!(
            os_release("IMAGE_ID=rift\nIMAGE_VERSION=0.1.0\n"),
            Some(("rift".to_string(), "0.1.0".to_string()))
        );
        assert_eq!(os_release("IMAGE_ID=rift\n"), None);
        assert_eq!(os_release("IMAGE_ID=rift\nIMAGE_VERSION=\"0.1 0\"\n"), None);
        assert_eq!(os_release("IMAGE_IDX=rift\nIMAGE_VERSION=0.1.0\n"), None);
    }

    /// What cryptsetup 2.8.7's `veritysetup status usr` prints for slot b's store.
    const STATUS: &str = "/dev/mapper/usr is active and is in use.\n  \
          type:        VERITY\n  \
          status:      verified\n  \
          hash type:   1\n  \
          data block:  512 [bytes]\n  \
          hash block:  512 [bytes]\n  \
          hash name:   sha256\n  \
          salt:        5a1e0f0c3b6d4e8fa2c1b0d9e8f7a6b5c4d3e2f1a0b9c8d7e6f5a4b3c2d1e0f9\n  \
          data device: /dev/nvme0n1p5\n  \
          size:        12163072 [512-byte units] (6227492864 [bytes])\n  \
          mode:        readonly\n  \
          hash device: /dev/nvme0n1p4\n  \
          hash offset: 1 [512-byte units] (512 [bytes])\n  \
          root hash:   0e5c6a7d8b9f0a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293\n  \
          flags:       panic_on_corruption\n";

    #[test]
    fn the_store_is_copied_as_far_as_dm_verity_reads_it() {
        assert_eq!(
            read_veritysetup(STATUS),
            Some(Verity {
                data: PathBuf::from("/dev/nvme0n1p5"),
                hash: PathBuf::from("/dev/nvme0n1p4"),
                bytes: 12_163_072 * 512,
            })
        );
        assert_eq!(size(12_163_072 * 512), "5.8 GiB");
        // older versions print the size as sectors
        let older = STATUS.replace(
            "12163072 [512-byte units] (6227492864 [bytes])",
            "12163072 sectors",
        );
        assert_eq!(read_veritysetup(&older), read_veritysetup(STATUS));
        // the hash offset and the data block are not the size or the devices
        assert_eq!(
            read_veritysetup(&STATUS.replace(
                "size:        12163072 [512-byte units] (6227492864 [bytes])\n",
                ""
            )),
            None
        );
        assert_eq!(
            read_veritysetup(
                &STATUS.replace("hash device: /dev/nvme0n1p4", "hash device: nvme0n1p4")
            ),
            None
        );
        assert_eq!(
            read_veritysetup(&STATUS.replace("12163072 [512", "0 [512")),
            None
        );
        assert_eq!(read_veritysetup("/dev/mapper/usr is inactive.\n"), None);
    }

    #[test]
    fn btrfs_sends_and_receives_the_paths_it_is_given() {
        let words = |args: Vec<OsString>| {
            args.into_iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            words(send_args(Path::new("/persist/@snapshots/clone/@home"))),
            ["send", "/persist/@snapshots/clone/@home"]
        );
        assert_eq!(
            words(receive_args(Path::new(
                "/run/vault-clone/persist-7/.received"
            ))),
            ["receive", "/run/vault-clone/persist-7/.received"]
        );
    }
}
