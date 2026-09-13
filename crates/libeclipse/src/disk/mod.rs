//! Drives: the layout Eclipse writes onto a disk, what lsblk, sfdisk, diskutil and Get-Disk say about
//! disks, which disks may be erased, and the programs that write one. Vault's clone and eclipse-flash
//! both write drives through this.
//!
//! Behind the `disk` feature, which brings in serde for what those programs print and getrandom.

mod diskutil;
mod getdisk;
mod gpt;
mod guard;
mod lsblk;
mod plist;
pub mod run;
mod table;

pub use diskutil::{read_diskutil, read_diskutil_info};
pub use getdisk::{GET_DISK, read_get_disk};
pub use gpt::{GPT_BYTES, Gpt, read_gpt, write_gpt};
pub use guard::{Bus, Disk, Volume};
pub use lsblk::{Block, LSBLK, confirmation, describe, disks_in, read_blocks, read_lsblk, refuse};
pub use table::{ALIGN, Partition, Slot, Table, plan, random_uuid, read_table, script};

/// A mebibyte.
pub const MIB: u64 = 1 << 20;
/// A gibibyte.
pub const GIB: u64 = 1 << 30;
/// The size of the esp on a drive.
pub const ESP_SIZE: u64 = GIB;
/// The size of each slot's verity partition.
pub const VERITY_SIZE: u64 = GIB;
/// The size of each slot's store partition.
pub const STORE_SIZE: u64 = 8 * GIB;
/// Alignment and the two copies of the partition table.
pub const SLACK: u64 = 64 * MIB;
/// Room for what persist holds now to grow into.
pub const HEADROOM: u64 = GIB;
/// The least persist gets.
pub const LEAST_PERSIST: u64 = 2 * GIB;
/// The logical sector size the system partitions are made for. dm-verity reads the store in blocks
/// of this size, and a disk with bigger sectors cannot map them.
pub const SECTOR: u64 = 512;

/// GPT partition types from the discoverable partitions specification, the way sfdisk prints them.
/// The esp.
pub const ESP_TYPE: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";
/// `/usr` on x86-64, the store.
pub const USR_TYPE: &str = "8484680C-9521-48C6-9C11-B0720656F69E";
/// The verity data of `/usr` on x86-64.
pub const USR_VERITY_TYPE: &str = "77FF5F63-E7B6-4633-ACF4-1565B864C0E6";
/// Linux data, persist.
pub const LINUX_TYPE: &str = "0FC63DAF-8483-4772-8E79-3D69D8477DE4";
/// Basic data, the exchange partition, which Windows and macOS mount.
pub const BASIC_DATA_TYPE: &str = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7";

/// The subvolumes of persist, as a new drive gets them.
pub const SUBVOLUMES: [&str; 6] = [
    "@home",
    "@var",
    "@flatpak",
    "@models",
    "@hosts",
    "@snapshots",
];
/// The mount options of persist that matter while it is written.
pub const PERSIST_OPTIONS: &str = "compress=zstd:3,noatime";
/// Where the system keeps its machine id, in `@var`.
pub const MACHINE_ID: &str = "lib/eclipse/machine-id";
/// The owner's account: the name of its home in `@home`, its user id and its group id.
pub const OWNER: (&str, u32, u32) = ("eclipse", 1000, 100);

/// How many bytes a drive needs: the esp, two slots, the exchange partition when there is one, and
/// persist with room over what it holds, at least 2 GiB.
#[must_use]
pub fn needed(persist_used: u64, exchange: Option<u64>) -> u64 {
    ESP_SIZE
        + 2 * (VERITY_SIZE + STORE_SIZE)
        + exchange.unwrap_or(0)
        + (persist_used + HEADROOM).max(LEAST_PERSIST)
        + SLACK
}

/// Bytes in GiB with one decimal, or in MiB below that.
#[must_use]
pub fn size(bytes: u64) -> String {
    if bytes >= GIB - MIB / 2 {
        let gib = u128::from(GIB);
        let tenths = (u128::from(bytes) * 10 + gib / 2) / gib;
        format!("{}.{} GiB", tenths / 10, tenths % 10)
    } else {
        format!("{} MiB", bytes.div_ceil(MIB))
    }
}

/// The device of partition `number` on `disk`: `/dev/sdb1`, or `/dev/nvme0n1p1` when the disk's
/// name ends in a digit.
#[must_use]
pub fn partition_node(disk: &str, number: usize) -> String {
    if disk.ends_with(|c: char| c.is_ascii_digit()) {
        format!("{disk}p{number}")
    } else {
        format!("{disk}{number}")
    }
}

/// A machine id for 16 random bytes: 32 lower case hex digits and a newline.
#[must_use]
pub fn machine_id(random: [u8; 16]) -> String {
    use std::fmt::Write as _;

    let mut text = String::with_capacity(33);
    for byte in random {
        let _ = write!(text, "{byte:02x}");
    }
    text.push('\n');
    text
}

/// What is wrong with a passphrase for persist, if anything.
#[must_use]
pub fn passphrase_problem(passphrase: &str) -> Option<&'static str> {
    (passphrase.chars().count() < 8).then_some("A passphrase needs at least 8 characters.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drive_needs_two_slots_and_room_for_persist() {
        // 1G esp, 2 x 9G slots, persist of at least 2G, 64M of slack
        assert_eq!(needed(0, None), 21 * GIB + 64 * MIB);
        assert_eq!(needed(GIB, None), 21 * GIB + 64 * MIB);
        assert_eq!(needed(10 * GIB, None), 30 * GIB + 64 * MIB);
        assert_eq!(needed(10 * GIB, Some(8 * GIB)), 38 * GIB + 64 * MIB);
    }

    #[test]
    fn partitions_are_named_the_way_the_kernel_names_them() {
        assert_eq!(partition_node("/dev/sdb", 1), "/dev/sdb1");
        assert_eq!(partition_node("/dev/sdb", 6), "/dev/sdb6");
        assert_eq!(partition_node("/dev/nvme1n1", 3), "/dev/nvme1n1p3");
        assert_eq!(partition_node("/dev/mmcblk0", 7), "/dev/mmcblk0p7");
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
    fn sizes_read_in_binary_units() {
        assert_eq!(size(512 * MIB), "512 MiB");
        assert_eq!(size(GIB), "1.0 GiB");
        assert_eq!(size(12_163_072 * 512), "5.8 GiB");
    }
}
