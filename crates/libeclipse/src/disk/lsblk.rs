//! What lsblk says about a disk, and the rules a disk has to pass before anything erases it.

use serde::Deserialize;

use super::guard::{Bus, Disk, Volume, reported};

/// What lsblk is asked about a disk and everything on it.
pub const LSBLK: &str =
    "NAME,PATH,TYPE,SIZE,RM,RO,TRAN,SERIAL,MODEL,MOUNTPOINTS,FSTYPE,PARTLABEL,LOG-SEC";

/// A disk, a partition or a device over one, from `lsblk --json --bytes`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Block {
    /// The kernel's name, `sdb`.
    pub name: String,
    /// `/dev/sdb`.
    pub path: String,
    /// `disk`, `part`, `crypt`, `loop` and so on.
    #[serde(rename = "type")]
    pub kind: String,
    /// In bytes.
    pub size: u64,
    /// Whether the kernel sees it as removable media.
    #[serde(default)]
    pub rm: bool,
    /// Whether it is read only.
    #[serde(default)]
    pub ro: bool,
    /// How it is attached: `usb`, `nvme`, `sata`.
    pub tran: Option<String>,
    /// The serial number the device reports.
    pub serial: Option<String>,
    /// The model the device reports.
    pub model: Option<String>,
    /// Where it is mounted, `[SWAP]` for swap.
    #[serde(default)]
    pub mountpoints: Vec<Option<String>>,
    /// The file system on it.
    pub fstype: Option<String>,
    /// The partition's label.
    pub partlabel: Option<String>,
    /// The logical sector size in bytes.
    #[serde(rename = "log-sec")]
    pub log_sec: Option<u64>,
    /// The partitions on it and the devices over it.
    #[serde(default)]
    pub children: Vec<Block>,
}

/// Every device in what lsblk printed, with what is on each.
///
/// # Errors
///
/// When lsblk printed something other than devices.
pub fn read_blocks(json: &str) -> Result<Vec<Block>, String> {
    #[derive(Deserialize)]
    struct Lsblk {
        blockdevices: Vec<Block>,
    }
    serde_json::from_str::<Lsblk>(json)
        .map(|found| found.blockdevices)
        .map_err(|e| format!("lsblk printed something else: {e}"))
}

/// The first device in what lsblk printed.
///
/// # Errors
///
/// When lsblk printed something other than a device.
pub fn read_lsblk(json: &str) -> Result<Block, String> {
    read_blocks(json)?
        .into_iter()
        .next()
        .ok_or_else(|| "lsblk printed no disk.".to_string())
}

/// The disks in what `lsblk --inverse --list --noheadings --output NAME,TYPE` printed for a device:
/// the ones it is on.
#[must_use]
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

impl Block {
    /// The disk as the rules see it. `running` holds the names of the disks the running system is
    /// on.
    #[must_use]
    pub fn disk(&self, running: &[String]) -> Disk {
        Disk {
            name: self.name.clone(),
            path: self.path.clone(),
            whole: self.kind == "disk",
            running: running.contains(&self.name),
            bus: if self.tran.as_deref() == Some("usb") {
                Bus::Usb
            } else if self.rm {
                Bus::Removable
            } else {
                Bus::Inside
            },
            read_only: self.ro,
            sector: self.log_sec,
            size: self.size,
            model: reported(self.model.as_deref()),
            serial: reported(self.serial.as_deref()),
            volumes: self
                .children
                .iter()
                .map(|child| Volume {
                    path: child.path.clone(),
                    name: reported(child.partlabel.as_deref())
                        .unwrap_or_else(|| child.name.clone()),
                    fs: child.fstype.clone(),
                    mounted: child.mountpoints.iter().flatten().next().cloned(),
                })
                .collect(),
        }
    }
}

/// Refuses a disk a drive must not be written onto. `running` holds the names of the disks the
/// running system is on, `needed` the bytes the drive needs. Nothing is unmounted or closed on Linux,
/// so a disk with anything in use on it is refused too.
///
/// # Errors
///
/// The sentence that says why the disk is refused.
pub fn refuse(disk: &Block, running: &[String], needed: u64) -> Result<(), String> {
    disk.disk(running).refuse(needed, in_use(disk).as_deref())
}

fn serial_of(disk: &Block) -> Option<&str> {
    disk.serial
        .as_deref()
        .map(str::trim)
        .filter(|serial| !serial.is_empty())
}

/// What a person is shown about the disk before it is erased.
#[must_use]
pub fn describe(disk: &Block) -> Vec<String> {
    disk.disk(&[]).describe()
}

/// What has to be typed back before the disk is erased: its serial, or its name when it has none.
#[must_use]
pub fn confirmation(disk: &Block) -> &str {
    serial_of(disk).unwrap_or(&disk.name)
}

#[cfg(test)]
mod tests {
    use super::super::{GIB, needed};
    use super::*;

    /// The disk the boot test clones onto: a qemu scsi disk with removable on, and nothing on it.
    const TEST_DISK: &str = r#"{
       "blockdevices": [
          {"name": "sda", "path": "/dev/sda", "type": "disk", "size": 25769803776, "rm": true, "ro": false,
           "tran": null, "serial": "clone", "model": "QEMU HARDDISK   ", "mountpoints": [null],
           "fstype": null, "partlabel": null, "log-sec": 512}
       ]
    }"#;

    /// A USB stick with an exfat partition that the desktop mounted.
    const STICK: &str = r#"{
       "blockdevices": [
          {"name": "sdb", "path": "/dev/sdb", "type": "disk", "size": 61530439680, "rm": false, "ro": false,
           "tran": "usb", "serial": "4C530001230717117401", "model": "Ultra Fit", "mountpoints": [null],
           "fstype": null, "partlabel": null, "log-sec": 512,
           "children": [
              {"name": "sdb1", "path": "/dev/sdb1", "type": "part", "size": 61529391104, "rm": false,
               "ro": false, "tran": null, "serial": null, "model": null,
               "mountpoints": ["/run/media/eclipse/STICK"], "fstype": "exfat", "partlabel": "Main Data Partition",
               "log-sec": 512}
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
        assert_eq!(disk.log_sec, Some(512));
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
    fn the_test_disk_passes_the_rules_real_disks_do() {
        let disk = read_lsblk(TEST_DISK).unwrap();
        let running = ["nvme0n1".to_string()];
        assert_eq!(refuse(&disk, &running, needed(GIB, None)), Ok(()));
        // a USB disk that does not say it is removable is fine too, once nothing on it is mounted
        let mut stick = stick();
        stick.children[0].mountpoints = vec![None];
        assert_eq!(refuse(&stick, &running, needed(GIB, None)), Ok(()));
        // an lsblk too old to know the sector size
        stick.log_sec = None;
        assert_eq!(refuse(&stick, &running, needed(GIB, None)), Ok(()));
    }

    #[test]
    fn disks_a_drive_must_not_be_written_onto_are_refused() {
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
        // a loop device is not a disk either
        partition.kind = "loop".into();
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
            log_sec: Some(512),
            children: Vec::new(),
        }];
        refused(&opened, "/dev/sdb1 is open as /dev/mapper/luks-1");

        let mut locked = read_lsblk(TEST_DISK).unwrap();
        locked.ro = true;
        refused(&locked, "read only");

        let mut big_sectors = read_lsblk(TEST_DISK).unwrap();
        big_sectors.log_sec = Some(4096);
        refused(&big_sectors, "has sectors of 4096 bytes");

        let mut small = read_lsblk(TEST_DISK).unwrap();
        small.size = 16 * GIB;
        refused(&small, "holds 16.0 GiB, and Eclipse needs 21.1 GiB");
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
}
