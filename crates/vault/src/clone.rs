//! Cloning: a second drive written onto a removable disk, that boots by itself and opens with a
//! passphrase of its own.
//!
//! The clone gets a new partition table. The running slot is copied as blocks into its slot A, its
//! slot B is empty, and its esp is new, with systemd-boot and the running version's uki. Persist is
//! new too: LUKS2 with a volume key of its own around a new btrfs, and every subvolume but the
//! snapshots is sent into it from a read-only snapshot of this drive's. Vault only writes onto a
//! whole disk that is removable or on USB, that nothing uses, and that the running system is not on.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;

use crate::backup::tool;
use crate::timeline;

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;
/// The sizes of the esp and of each slot's two partitions, the same as on a drive the flasher writes.
const ESP_SIZE: u64 = GIB;
const VERITY_SIZE: u64 = GIB;
const STORE_SIZE: u64 = 8 * GIB;
/// Alignment and the two copies of the partition table.
const SLACK: u64 = 64 * MIB;
/// Room for what persist holds now to grow into, and the least persist gets.
const HEADROOM: u64 = GIB;
const LEAST_PERSIST: u64 = 2 * GIB;
/// How much of a partition is copied at a time.
const CHUNK: usize = 4 << 20;

/// GPT partition types from the discoverable partitions specification, the way sfdisk prints them.
const ESP_TYPE: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";
const USR_TYPE: &str = "8484680C-9521-48C6-9C11-B0720656F69E";
const USR_VERITY_TYPE: &str = "77FF5F63-E7B6-4633-ACF4-1565B864C0E6";
const LINUX_TYPE: &str = "0FC63DAF-8483-4772-8E79-3D69D8477DE4";
const BASIC_DATA_TYPE: &str = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7";

/// The subvolumes a clone gets a copy of. `@snapshots` is made new and empty.
pub const SUBVOLUMES: [&str; 5] = ["@home", "@var", "@flatpak", "@models", "@hosts"];
/// Files in `@var` the system made for this drive alone. The clone makes its own.
const FORGET: [&str; 3] = [
    "lib/systemd/random-seed",
    "lib/systemd/credential.secret",
    "lib/NetworkManager/secret_key",
];
const MACHINE_ID: &str = "lib/eclipse/machine-id";
/// What the esp needs besides the uki.
const BOOT_FILES: [&str; 3] = [
    "EFI/BOOT/BOOTX64.EFI",
    "EFI/systemd/systemd-bootx64.efi",
    "loader/loader.conf",
];
/// The mount options of persist that matter while it is filled.
const PERSIST_OPTIONS: &str = "compress=zstd:3,noatime";
/// What lsblk says about a disk and everything on it.
const LSBLK: &str = "NAME,PATH,TYPE,SIZE,RM,RO,TRAN,SERIAL,MODEL,MOUNTPOINTS,FSTYPE,PARTLABEL";
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

/// A disk, a partition or a device over one, from `lsblk --json --bytes`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Block {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub size: u64,
    #[serde(default)]
    pub rm: bool,
    #[serde(default)]
    pub ro: bool,
    pub tran: Option<String>,
    pub serial: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub mountpoints: Vec<Option<String>>,
    pub fstype: Option<String>,
    pub partlabel: Option<String>,
    #[serde(default)]
    pub children: Vec<Block>,
}

/// The first device in what lsblk printed.
pub fn read_lsblk(json: &str) -> Result<Block, String> {
    #[derive(Deserialize)]
    struct Lsblk {
        blockdevices: Vec<Block>,
    }
    let found: Lsblk =
        serde_json::from_str(json).map_err(|e| format!("lsblk printed something else: {e}"))?;
    found
        .blockdevices
        .into_iter()
        .next()
        .ok_or_else(|| "lsblk printed no disk.".to_string())
}

/// The disks in what `lsblk --inverse --list --noheadings --output NAME,TYPE` printed for a device:
/// the ones it is on.
pub fn disks_in(list: &str) -> Vec<String> {
    list.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            match (words.next(), words.next()) {
                (Some(name), Some("disk")) => Some(name.to_string()),
                _ => None,
            }
        })
        .collect()
}

/// The first thing on `block` or under it that is in use: a mountpoint, swap, or a device opened
/// over it.
fn in_use(block: &Block) -> Option<String> {
    if let Some(point) = block.mountpoints.iter().flatten().next() {
        return Some(if point == "[SWAP]" {
            format!("{} is swap", block.path)
        } else {
            format!("{} is mounted on {point}", block.path)
        });
    }
    block.children.iter().find_map(|child| {
        if child.kind == "part" {
            in_use(child)
        } else {
            Some(format!("{} is open as {}", block.path, child.path))
        }
    })
}

/// How many bytes a clone needs: the esp, two slots, the exchange partition when the drive has one,
/// and persist with room over what it holds now.
pub fn needed(persist_used: u64, exchange: Option<u64>) -> u64 {
    ESP_SIZE
        + 2 * (VERITY_SIZE + STORE_SIZE)
        + exchange.unwrap_or(0)
        + (persist_used + HEADROOM).max(LEAST_PERSIST)
        + SLACK
}

/// Refuses a disk a clone must not be written onto. `running` holds the names of the disks the
/// running system is on, `needed` the bytes the clone needs.
pub fn refuse(disk: &Block, running: &[String], needed: u64) -> Result<(), String> {
    let path = &disk.path;
    if disk.kind != "disk" {
        return Err(format!(
            "{path} is not a whole disk. Give the disk itself, not a partition or a device on it."
        ));
    }
    if running.contains(&disk.name) {
        return Err(format!("{path} is the drive this system runs from."));
    }
    if !(disk.rm || disk.tran.as_deref() == Some("usb")) {
        return Err(format!(
            "{path} is neither removable nor on USB. Vault only clones onto a stick or a USB disk, so a disk inside a computer is never erased."
        ));
    }
    if disk.ro {
        return Err(format!("{path} is read only."));
    }
    if let Some(why) = in_use(disk) {
        return Err(format!("{why}. Unmount or close it first."));
    }
    if disk.size < needed {
        return Err(format!(
            "{path} holds {}, and the clone needs {}.",
            size(disk.size),
            size(needed)
        ));
    }
    Ok(())
}

/// A partition in what `sfdisk --json` printed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Partition {
    pub node: String,
    /// In sectors.
    pub size: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub uuid: Option<String>,
    pub name: Option<String>,
    pub attrs: Option<String>,
}

/// A partition table from `sfdisk --json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Table {
    #[serde(default = "sector")]
    pub sectorsize: u64,
    pub partitions: Vec<Partition>,
}

fn sector() -> u64 {
    512
}

impl Table {
    pub fn bytes(&self, partition: &Partition) -> u64 {
        partition.size * self.sectorsize
    }

    pub fn named(&self, name: &str) -> Option<&Partition> {
        self.partitions
            .iter()
            .find(|partition| partition.name.as_deref() == Some(name))
    }
}

pub fn read_table(json: &str) -> Result<Table, String> {
    #[derive(Deserialize)]
    struct Sfdisk {
        partitiontable: Table,
    }
    serde_json::from_str::<Sfdisk>(json)
        .map(|found| found.partitiontable)
        .map_err(|e| format!("sfdisk printed a partition table Vault cannot read: {e}"))
}

/// The slot the running system is on: its version and its two partitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub version: String,
    pub verity: Partition,
    pub store: Partition,
}

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

/// The sfdisk script for the clone: the esp, the running slot as slot A with the uuids its uki
/// looks for, an empty slot B, the exchange partition when the drive has one, and persist in the
/// rest.
pub fn script(slot: &Slot, exchange: Option<u64>) -> String {
    let mut lines = vec![
        "label: gpt".to_string(),
        format!("size={}MiB, type={ESP_TYPE}, name=\"esp\"", ESP_SIZE / MIB),
    ];
    for (partition, bytes) in [(&slot.verity, VERITY_SIZE), (&slot.store, STORE_SIZE)] {
        let mut fields = vec![
            format!("size={}MiB", bytes / MIB),
            format!("type={}", partition.kind),
        ];
        if let Some(uuid) = &partition.uuid {
            fields.push(format!("uuid={uuid}"));
        }
        if let Some(name) = &partition.name {
            fields.push(format!("name=\"{name}\""));
        }
        if let Some(attrs) = partition.attrs.as_deref().filter(|attrs| !attrs.is_empty()) {
            fields.push(format!("attrs=\"{attrs}\""));
        }
        lines.push(fields.join(", "));
    }
    lines.push(format!(
        "size={}MiB, type={USR_VERITY_TYPE}, name=\"_empty\"",
        VERITY_SIZE / MIB
    ));
    lines.push(format!(
        "size={}MiB, type={USR_TYPE}, name=\"_empty\"",
        STORE_SIZE / MIB
    ));
    if let Some(bytes) = exchange {
        lines.push(format!(
            "size={}MiB, type={BASIC_DATA_TYPE}, name=\"exchange\"",
            bytes.div_ceil(MIB)
        ));
    }
    lines.push(format!("type={LINUX_TYPE}, name=\"persist\""));
    lines.join("\n") + "\n"
}

/// The device of partition `number` on `disk`: `/dev/sdb1`, or `/dev/nvme0n1p1` when the disk's
/// name ends in a digit.
pub fn partition_node(disk: &str, number: usize) -> String {
    if disk.ends_with(|c: char| c.is_ascii_digit()) {
        format!("{disk}p{number}")
    } else {
        format!("{disk}{number}")
    }
}

/// The uki of `version` among the file names in the esp's `EFI/Linux`: `eclipse_0.2.0.efi`, or one
/// with a boot counter, `eclipse_0.2.0+2.efi` or `eclipse_0.2.0+1-2.efi`. The one without a counter
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
/// in sectors of 512 bytes, and nothing past it is ever read.
pub fn read_veritysetup(status: &str) -> Option<Verity> {
    let field = |name: &str| {
        status.lines().find_map(|line| {
            line.trim()
                .strip_prefix(name)?
                .strip_prefix(':')
                .map(str::trim)
        })
    };
    let sectors: u64 = field("size")?
        .strip_suffix("sectors")?
        .trim()
        .parse()
        .ok()?;
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

/// A machine id for 16 random bytes: 32 lower case hex digits and a newline.
pub fn machine_id(random: [u8; 16]) -> String {
    use std::fmt::Write as _;

    let mut text = String::with_capacity(33);
    for byte in random {
        let _ = write!(text, "{byte:02x}");
    }
    text.push('\n');
    text
}

/// What is wrong with a passphrase for the clone, if anything.
pub fn passphrase_problem(passphrase: &str) -> Option<&'static str> {
    (passphrase.chars().count() < 8).then_some("A passphrase needs at least 8 characters.")
}

fn serial_of(disk: &Block) -> Option<&str> {
    disk.serial
        .as_deref()
        .map(str::trim)
        .filter(|serial| !serial.is_empty())
}

/// What a person is shown about the disk before it is erased.
pub fn describe(disk: &Block) -> Vec<String> {
    let mut about = vec![disk.path.clone()];
    if let Some(model) = disk
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        about.push(model.to_string());
    }
    about.push(size(disk.size));
    if let Some(serial) = serial_of(disk) {
        about.push(format!("serial {serial}"));
    }
    let parts: Vec<String> = disk
        .children
        .iter()
        .map(|child| {
            let name = child
                .partlabel
                .as_deref()
                .filter(|label| !label.is_empty())
                .unwrap_or(&child.name);
            match child.fstype.as_deref() {
                Some(fstype) => format!("{name} ({fstype})"),
                None => name.to_string(),
            }
        })
        .collect();
    vec![
        format!("{}.", about.join(", ")),
        if parts.is_empty() {
            "It has no partitions.".to_string()
        } else {
            format!("It holds {}.", parts.join(", "))
        },
    ]
}

/// What has to be typed back before the disk is erased: its serial, or its name when it has none.
pub fn confirmation(disk: &Block) -> &str {
    serial_of(disk).unwrap_or(&disk.name)
}

/// Bytes in GiB with one decimal, or in MiB below that.
pub fn size(bytes: u64) -> String {
    if bytes >= GIB - MIB / 2 {
        let gib = u128::from(GIB);
        let tenths = (u128::from(bytes) * 10 + gib / 2) / gib;
        format!("{}.{} GiB", tenths / 10, tenths % 10)
    } else {
        format!("{} MiB", bytes.div_ceil(MIB))
    }
}

/// sfdisk wipes what was on the disk and on the new partitions, so nothing old is found in them.
pub fn sfdisk_args(disk: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["--wipe", "always", "--wipe-partitions", "always", "--quiet"]
        .map(OsString::from)
        .into();
    args.push(disk.into());
    args
}

/// A new LUKS2 header, and with it a new volume key. The passphrase comes on stdin, as it is.
pub fn format_args(partition: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "luksFormat",
        "--type",
        "luks2",
        "--batch-mode",
        "--label",
        "persist",
        "--key-file",
        "-",
    ]
    .map(OsString::from)
    .into();
    args.push(partition.into());
    args
}

pub fn open_args(partition: &Path, name: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["open", "--key-file", "-"].map(OsString::from).into();
    args.extend([partition.into(), name.into()]);
    args
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

/// A file system mounted until this is dropped or unmounted.
struct Mounted {
    path: PathBuf,
    done: bool,
}

impl Mounted {
    fn new(device: &Path, path: PathBuf, options: Option<&str>) -> Result<Mounted, String> {
        fs::create_dir_all(&path).map_err(|e| format!("Could not make {}: {e}", path.display()))?;
        let mut command = Command::new("mount");
        if let Some(options) = options {
            command.args(["-o", options]);
        }
        tool(command.arg(device).arg(&path)).inspect_err(|_| {
            let _ = fs::remove_dir(&path);
        })?;
        Ok(Mounted { path, done: false })
    }

    fn unmount(mut self) -> Result<(), String> {
        self.done = true;
        tool(Command::new("umount").arg(&self.path))?;
        let _ = fs::remove_dir(&self.path);
        Ok(())
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        if !self.done {
            let _ = Command::new("umount").arg(&self.path).output();
            let _ = fs::remove_dir(&self.path);
        }
    }
}

/// An opened LUKS volume, closed when this is dropped or closed.
struct Opened {
    name: String,
    done: bool,
}

impl Opened {
    fn close(mut self) -> Result<(), String> {
        self.done = true;
        tool(Command::new("cryptsetup").args(["close", &self.name])).map(|_| ())
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        if !self.done {
            let _ = Command::new("cryptsetup")
                .args(["close", &self.name])
                .output();
        }
    }
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
            Command::new("sfdisk").args(sfdisk_args(Path::new(disk))),
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
            &running.verity,
            &part(2),
            running.verity_bytes,
            &mut |_: String| {},
        )?;
        copy(&running.store, &part(3), running.data, say)?;
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
            None,
        )?;
        for file in BOOT_FILES {
            copy_file(&self.boot.join(file), &esp.path.join(file))?;
        }
        copy_file(
            &running.uki,
            &esp.path.join("EFI/Linux").join(&running.uki_name),
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
        feed(
            Command::new("cryptsetup").args(format_args(partition)),
            passphrase,
        )?;
        let name = format!("vault-clone-{}", std::process::id());
        feed(
            Command::new("cryptsetup").args(open_args(partition, &name)),
            passphrase,
        )?;
        let opened = Opened {
            name: name.clone(),
            done: false,
        };
        let mapper = Path::new("/dev/mapper").join(&name);
        tool(
            Command::new("mkfs.btrfs")
                .args(["-q", "-L", "persist"])
                .arg(&mapper),
        )?;
        let top = Mounted::new(
            &mapper,
            self.run.join(format!("persist-{}", std::process::id())),
            Some(PERSIST_OPTIONS),
        )?;
        self.fill(&top.path, say)?;
        top.unmount()?;
        opened.close()
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
        fs::write(&id, machine_id(random()?))
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

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
}

/// The bytes persist holds now.
fn used(persist: &Path) -> Result<u64, String> {
    let stat = rustix::fs::statvfs(persist)
        .map_err(|e| format!("Could not read how full {} is: {e}", persist.display()))?;
    Ok(stat.f_blocks.saturating_sub(stat.f_bfree) * stat.f_frsize)
}

fn random() -> Result<[u8; 16], String> {
    let mut random = [0; 16];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut random))
        .map_err(|e| format!("Could not get random numbers: {e}"))?;
    Ok(random)
}

/// Waits for udev to make the new partitions' devices.
fn settle(nodes: &[PathBuf]) -> Result<(), String> {
    for _ in 0..50 {
        let _ = Command::new("udevadm")
            .args(["settle", "--timeout", "10"])
            .output();
        if nodes.iter().all(|node| node.exists()) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!(
        "The new partitions did not show up: {}",
        nodes
            .iter()
            .filter(|node| !node.exists())
            .map(|node| node.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Copies the first `bytes` of `from` onto `to`, saying how far it is at each tenth.
fn copy(from: &Path, to: &Path, bytes: u64, say: &mut impl FnMut(String)) -> Result<(), String> {
    let reading = |e: io::Error| format!("Could not read {}: {e}", from.display());
    let writing = |e: io::Error| format!("Could not write {}: {e}", to.display());
    let mut source = File::open(from).map_err(reading)?;
    let mut target = OpenOptions::new().write(true).open(to).map_err(writing)?;
    let mut buffer = vec![0; CHUNK];
    let mut done = 0;
    let mut tenth = 1;
    while done < bytes {
        let want = usize::try_from(bytes - done).map_or(CHUNK, |left| left.min(CHUNK));
        source.read_exact(&mut buffer[..want]).map_err(reading)?;
        target.write_all(&buffer[..want]).map_err(writing)?;
        done += u64::try_from(want).unwrap_or(u64::MAX);
        if tenth < 10 && done * 10 >= bytes * tenth {
            // a stick takes what is written into its cache fast and writes it out slowly
            target.sync_data().map_err(writing)?;
            say(format!("Copied {} of {}.", size(done), size(bytes)));
            while tenth < 10 && done * 10 >= bytes * tenth {
                tenth += 1;
            }
        }
    }
    target.sync_all().map_err(writing)
}

fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Could not make {}: {e}", parent.display()))?;
    }
    File::open(from)
        .and_then(|mut source| {
            let mut target = File::create(to)?;
            io::copy(&mut source, &mut target)?;
            target.sync_all()
        })
        .map_err(|e| format!("Could not copy {} to {}: {e}", from.display(), to.display()))
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

/// Runs a program that has to succeed with `input` on its stdin. The input is never part of what
/// an error says.
fn feed(command: &mut Command, input: &str) -> Result<(), String> {
    let line = format!("{command:?}");
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not run {line}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.as_bytes())
            .map_err(|e| format!("Could not give {line} its input: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("{line} stopped: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{line} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The disk the boot test clones onto: a qemu scsi disk with removable on, and nothing on it.
    const TEST_DISK: &str = r#"{
       "blockdevices": [
          {"name": "sda", "path": "/dev/sda", "type": "disk", "size": 25769803776, "rm": true, "ro": false,
           "tran": null, "serial": "clone", "model": "QEMU HARDDISK   ", "mountpoints": [null],
           "fstype": null, "partlabel": null}
       ]
    }"#;

    /// A USB stick with an exfat partition that the desktop mounted.
    const STICK: &str = r#"{
       "blockdevices": [
          {"name": "sdb", "path": "/dev/sdb", "type": "disk", "size": 61530439680, "rm": false, "ro": false,
           "tran": "usb", "serial": "4C530001230717117401", "model": "Ultra Fit", "mountpoints": [null],
           "fstype": null, "partlabel": null,
           "children": [
              {"name": "sdb1", "path": "/dev/sdb1", "type": "part", "size": 61529391104, "rm": false,
               "ro": false, "tran": null, "serial": null, "model": null,
               "mountpoints": ["/run/media/eclipse/STICK"], "fstype": "exfat", "partlabel": "Main Data Partition"}
           ]}
       ]
    }"#;

    fn stick() -> Block {
        read_lsblk(STICK).unwrap()
    }

    #[test]
    fn lsblk_is_read_with_its_children() {
        let disk = read_lsblk(TEST_DISK).unwrap();
        assert_eq!(disk.path, "/dev/sda");
        assert!(disk.rm && !disk.ro);
        assert_eq!(disk.size, 24 * GIB);
        assert!(disk.children.is_empty());
        let stick = stick();
        assert_eq!(stick.tran.as_deref(), Some("usb"));
        assert_eq!(
            stick.children[0].mountpoints,
            [Some("/run/media/eclipse/STICK".to_string())]
        );
        assert!(read_lsblk(r#"{"blockdevices": []}"#).is_err());
        assert!(read_lsblk("lsblk: /dev/sdz: not a block device").is_err());
    }

    #[test]
    fn the_disks_a_device_is_on() {
        let list = "nvme0n1p1 part\nnvme0n1   disk\n";
        assert_eq!(disks_in(list), ["nvme0n1"]);
        let verity = "usr         crypt\nnvme0n1p2   part\nnvme0n1     disk\nnvme0n1p3   part\nnvme0n1     disk\n";
        assert_eq!(disks_in(verity), ["nvme0n1", "nvme0n1"]);
        assert!(disks_in("").is_empty());
    }

    #[test]
    fn a_clone_needs_two_slots_and_room_for_persist() {
        // 1G esp, 2 x 9G slots, persist of at least 2G, 64M of slack
        assert_eq!(needed(0, None), 21 * GIB + 64 * MIB);
        assert_eq!(needed(GIB, None), 21 * GIB + 64 * MIB);
        assert_eq!(needed(10 * GIB, None), 30 * GIB + 64 * MIB);
        assert_eq!(needed(10 * GIB, Some(8 * GIB)), 38 * GIB + 64 * MIB);
    }

    #[test]
    fn the_test_disk_passes_the_rules_real_disks_do() {
        let disk = read_lsblk(TEST_DISK).unwrap();
        let running = ["nvme0n1".to_string()];
        assert_eq!(refuse(&disk, &running, needed(GIB, None)), Ok(()));
        // a USB disk that does not say it is removable is fine too, once nothing on it is mounted
        let mut stick = stick();
        stick.children[0].mountpoints = vec![None];
        assert_eq!(refuse(&stick, &running, needed(GIB, None)), Ok(()));
    }

    #[test]
    fn disks_a_clone_must_not_touch_are_refused() {
        let running = ["nvme0n1".to_string()];
        let need = needed(GIB, None);
        let refused = |disk: &Block, words: &str| {
            let why = refuse(disk, &running, need).unwrap_err();
            assert!(why.contains(words), "{why}");
        };

        // a disk inside the computer
        let mut internal = read_lsblk(TEST_DISK).unwrap();
        internal.rm = false;
        internal.tran = Some("nvme".into());
        refused(&internal, "neither removable nor on USB");
        internal.tran = Some("sata".into());
        refused(&internal, "neither removable nor on USB");

        // the drive this system runs from, even when it is a stick
        let mut own = stick();
        own.name = "nvme0n1".into();
        refused(&own, "the drive this system runs from");

        // a partition, not the disk
        let mut partition = read_lsblk(TEST_DISK).unwrap();
        partition.kind = "part".into();
        refused(&partition, "not a whole disk");

        refused(&stick(), "/dev/sdb1 is mounted on /run/media/eclipse/STICK");

        let mut swap = stick();
        swap.children[0].mountpoints = vec![Some("[SWAP]".into())];
        refused(&swap, "/dev/sdb1 is swap");

        let mut opened = stick();
        opened.children[0].mountpoints = vec![None];
        opened.children[0].children = vec![Block {
            name: "luks-1".into(),
            path: "/dev/mapper/luks-1".into(),
            kind: "crypt".into(),
            size: GIB,
            rm: false,
            ro: false,
            tran: None,
            serial: None,
            model: None,
            mountpoints: vec![None],
            fstype: None,
            partlabel: None,
            children: Vec::new(),
        }];
        refused(&opened, "/dev/sdb1 is open as /dev/mapper/luks-1");

        let mut locked = read_lsblk(TEST_DISK).unwrap();
        locked.ro = true;
        refused(&locked, "read only");

        let mut small = read_lsblk(TEST_DISK).unwrap();
        small.size = 16 * GIB;
        refused(&small, "holds 16.0 GiB, and the clone needs 21.1 GiB");
    }

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
        assert_eq!(table.bytes(table.named("esp").unwrap()), GIB);
        assert_eq!(table.named("exchange"), None);
        let slot = running_slot(&table, "/dev/nvme0n1p4", "/dev/nvme0n1p5", "0.2.0").unwrap();
        assert_eq!(slot.version, "0.2.0");
        assert_eq!(
            slot.verity.uuid.as_deref(),
            Some("5C1D8E0A-2B3C-4D5E-8F90-A1B2C3D4E5F6")
        );
        assert_eq!(slot.store.name.as_deref(), Some("store_0.2.0"));

        // os-release says 0.3.0 while 0.2.0's partitions run
        let why = running_slot(&table, "/dev/nvme0n1p4", "/dev/nvme0n1p5", "0.3.0").unwrap_err();
        assert!(
            why.contains("labelled store-verity_0.2.0, not store-verity_0.3.0"),
            "{why}"
        );
        assert!(running_slot(&table, "/dev/sda2", "/dev/sda3", "0.2.0").is_err());
        assert!(read_table("[]").is_err());
    }

    #[test]
    fn the_clone_gets_the_running_slot_as_slot_a_and_an_empty_slot_b() {
        let table = read_table(TABLE).unwrap();
        let slot = running_slot(&table, "/dev/nvme0n1p4", "/dev/nvme0n1p5", "0.2.0").unwrap();
        assert_eq!(
            script(&slot, None),
            "label: gpt\n\
             size=1024MiB, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=\"esp\"\n\
             size=1024MiB, type=77FF5F63-E7B6-4633-ACF4-1565B864C0E6, uuid=5C1D8E0A-2B3C-4D5E-8F90-A1B2C3D4E5F6, name=\"store-verity_0.2.0\", attrs=\"GUID:60\"\n\
             size=8192MiB, type=8484680C-9521-48C6-9C11-B0720656F69E, uuid=7E8F9A0B-1C2D-4E3F-9051-627384950A1B, name=\"store_0.2.0\"\n\
             size=1024MiB, type=77FF5F63-E7B6-4633-ACF4-1565B864C0E6, name=\"_empty\"\n\
             size=8192MiB, type=8484680C-9521-48C6-9C11-B0720656F69E, name=\"_empty\"\n\
             type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name=\"persist\"\n"
        );
        let with_exchange = script(&slot, Some(8 * GIB));
        let lines: Vec<&str> = with_exchange.lines().collect();
        assert_eq!(lines.len(), 8);
        assert_eq!(
            lines[6],
            "size=8192MiB, type=EBD0A0A2-B9E5-4433-87C0-68B6B72699C7, name=\"exchange\""
        );
        assert!(lines[7].ends_with("name=\"persist\""));
    }

    #[test]
    fn partitions_are_named_the_way_the_kernel_names_them() {
        assert_eq!(partition_node("/dev/sdb", 1), "/dev/sdb1");
        assert_eq!(partition_node("/dev/sdb", 6), "/dev/sdb6");
        assert_eq!(partition_node("/dev/nvme1n1", 3), "/dev/nvme1n1p3");
        assert_eq!(partition_node("/dev/mmcblk0", 7), "/dev/mmcblk0p7");
    }

    #[test]
    fn the_running_uki_is_found_with_or_without_a_counter() {
        let names = |list: &[&str]| list.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            uki(
                &names(&["eclipse_0.2.0.efi", "eclipse_0.3.0+0-3.efi"]),
                "eclipse",
                "0.2.0"
            )
            .as_deref(),
            Some("eclipse_0.2.0.efi")
        );
        assert_eq!(
            uki(&names(&["eclipse_0.1.0+2-1.efi"]), "eclipse", "0.1.0").as_deref(),
            Some("eclipse_0.1.0+2-1.efi")
        );
        assert_eq!(
            uki(&names(&["eclipse_0.1.0+3.efi"]), "eclipse", "0.1.0").as_deref(),
            Some("eclipse_0.1.0+3.efi")
        );
        for other in [
            "eclipse_0.1.0.1.efi",
            "eclipse_0.1.0+.efi",
            "eclipse_0.1.0+a-1.efi",
            "eclipse_0.1.0+1-.efi",
            "eclipse_0.1.0.efi.bak",
            "other_0.1.0.efi",
        ] {
            assert_eq!(uki(&names(&[other]), "eclipse", "0.1.0"), None, "{other}");
        }
    }

    #[test]
    fn the_image_id_and_version_come_from_os_release() {
        let text =
            "NAME=NixOS\nIMAGE_ID=\"eclipse\"\nIMAGE_VERSION=\"0.2.0\"\nVERSION_ID=\"26.05\"\n";
        assert_eq!(
            os_release(text),
            Some(("eclipse".to_string(), "0.2.0".to_string()))
        );
        assert_eq!(
            os_release("IMAGE_ID=eclipse\nIMAGE_VERSION=0.1.0\n"),
            Some(("eclipse".to_string(), "0.1.0".to_string()))
        );
        assert_eq!(os_release("IMAGE_ID=eclipse\n"), None);
        assert_eq!(
            os_release("IMAGE_ID=eclipse\nIMAGE_VERSION=\"0.1 0\"\n"),
            None
        );
        assert_eq!(os_release("IMAGE_IDX=eclipse\nIMAGE_VERSION=0.1.0\n"), None);
    }

    /// What `veritysetup status usr` prints for slot b's store.
    const STATUS: &str = "/dev/mapper/usr is active and is in use.\n\
          type:        VERITY\n\
          status:      verified\n\
          hash type:   1\n\
          data block:  512\n\
          hash block:  512\n\
          hash name:   sha256\n\
          salt:        5a1e0f0c3b6d4e8fa2c1b0d9e8f7a6b5c4d3e2f1a0b9c8d7e6f5a4b3c2d1e0f9\n\
          data device: /dev/nvme0n1p5\n\
          size:        12163072 sectors\n\
          mode:        readonly\n\
          hash device: /dev/nvme0n1p4\n\
          hash offset: 1 sectors\n\
          root hash:   0e5c6a7d8b9f0a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293\n\
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
        // the hash offset and the data block are not the size or the devices
        assert_eq!(
            read_veritysetup(&STATUS.replace("size:        12163072 sectors\n", "")),
            None
        );
        assert_eq!(
            read_veritysetup(
                &STATUS.replace("hash device: /dev/nvme0n1p4", "hash device: nvme0n1p4")
            ),
            None
        );
        assert_eq!(
            read_veritysetup(&STATUS.replace("12163072 sectors", "0 sectors")),
            None
        );
        assert_eq!(read_veritysetup("/dev/mapper/usr is inactive.\n"), None);
    }

    #[test]
    fn a_machine_id_is_32_hex_digits() {
        assert_eq!(machine_id([0; 16]), "00000000000000000000000000000000\n");
        let mut random = [0xab; 16];
        random[15] = 0x01;
        assert_eq!(machine_id(random), "ababababababababababababababab01\n");
    }

    #[test]
    fn a_passphrase_has_at_least_8_characters() {
        assert!(passphrase_problem("").is_some());
        assert!(passphrase_problem("seven77").is_some());
        assert_eq!(passphrase_problem("eight888"), None);
        // characters, not bytes
        assert!(passphrase_problem(&"\u{e9}".repeat(7)).is_some());
    }

    #[test]
    fn the_disk_is_shown_before_it_is_erased() {
        assert_eq!(
            describe(&stick()),
            [
                "/dev/sdb, Ultra Fit, 57.3 GiB, serial 4C530001230717117401.",
                "It holds Main Data Partition (exfat)."
            ]
        );
        assert_eq!(confirmation(&stick()), "4C530001230717117401");
        let disk = read_lsblk(TEST_DISK).unwrap();
        assert_eq!(
            describe(&disk),
            [
                "/dev/sda, QEMU HARDDISK, 24.0 GiB, serial clone.",
                "It has no partitions."
            ]
        );
        let mut nameless = disk;
        nameless.serial = Some("   ".into());
        assert_eq!(confirmation(&nameless), "sda");
    }

    #[test]
    fn sizes_read_in_binary_units() {
        assert_eq!(size(512 * MIB), "512 MiB");
        assert_eq!(size(GIB), "1.0 GiB");
        assert_eq!(size(12_163_072 * 512), "5.8 GiB");
    }

    #[test]
    fn the_tools_get_the_disk_and_never_the_passphrase() {
        let words = |args: Vec<OsString>| {
            args.into_iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            words(sfdisk_args(Path::new("/dev/sda"))),
            [
                "--wipe",
                "always",
                "--wipe-partitions",
                "always",
                "--quiet",
                "/dev/sda"
            ]
        );
        assert_eq!(
            words(format_args(Path::new("/dev/sda6"))),
            [
                "luksFormat",
                "--type",
                "luks2",
                "--batch-mode",
                "--label",
                "persist",
                "--key-file",
                "-",
                "/dev/sda6"
            ]
        );
        assert_eq!(
            words(open_args(Path::new("/dev/sda6"), "vault-clone-7")),
            ["open", "--key-file", "-", "/dev/sda6", "vault-clone-7"]
        );
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
