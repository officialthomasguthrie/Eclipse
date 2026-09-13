//! The rules a disk has to pass before a drive is written onto it, the same on Linux, macOS and
//! Windows, and what a person is shown about the disk first.

use super::{SECTOR, size};

/// How a disk is attached, as far as the rules go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    /// On USB.
    Usb,
    /// Removable media that is not on USB, like a card in a reader.
    Removable,
    /// A disk image: a file the system shows as a disk.
    Image,
    /// None of those: a disk inside the computer.
    Inside,
}

/// A partition or a volume on a disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// Its device, `/dev/sdb1`, or what stands for one: `Partition 1 of \\.\PhysicalDrive2`.
    pub path: String,
    /// What it is shown as: its label, or its device's name.
    pub name: String,
    /// Its file system.
    pub fs: Option<String>,
    /// Where it is mounted.
    pub mounted: Option<String>,
}

/// A disk as the rules see it, on any system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    /// The system's name for it: `sdb`, `disk4`, `PhysicalDrive2`.
    pub name: String,
    /// What it is opened as: `/dev/sdb`, `/dev/disk4`, `\\.\PhysicalDrive2`.
    pub path: String,
    /// Whether it is a whole disk, not a partition or a device made over one.
    pub whole: bool,
    /// Whether the running system is on it.
    pub running: bool,
    /// How it is attached.
    pub bus: Bus,
    /// Whether it is read only.
    pub read_only: bool,
    /// Its logical sector size in bytes, when the system says.
    pub sector: Option<u64>,
    /// In bytes.
    pub size: u64,
    /// The model the device reports.
    pub model: Option<String>,
    /// The serial number the device reports.
    pub serial: Option<String>,
    /// What is on it.
    pub volumes: Vec<Volume>,
}

impl Disk {
    /// Whether `eclipse-flash list` shows it: a whole disk the system does not run from, on USB,
    /// removable, or a disk image with nothing on it mounted.
    #[must_use]
    pub fn listed(&self) -> bool {
        self.whole
            && !self.running
            && match self.bus {
                Bus::Usb | Bus::Removable => true,
                Bus::Image => self.mounted().is_none(),
                Bus::Inside => false,
            }
    }

    /// The first volume on it that is mounted, said as the start of a sentence.
    #[must_use]
    pub fn mounted(&self) -> Option<String> {
        self.volumes.iter().find_map(|volume| {
            volume
                .mounted
                .as_ref()
                .map(|point| format!("{} is mounted on {point}", volume.path))
        })
    }

    /// Refuses a disk a drive must not be written onto. `needed` holds the bytes the drive needs,
    /// `in_use` what on the disk is in use and is not let go of for the person: anything mounted or
    /// opened on Linux, what is mounted from a disk image on macOS and Windows.
    ///
    /// # Errors
    ///
    /// The sentence that says why the disk is refused.
    pub fn refuse(&self, needed: u64, in_use: Option<&str>) -> Result<(), String> {
        let path = &self.path;
        if !self.whole {
            return Err(format!(
                "{path} is not a whole disk. Give the disk itself, not a partition or a device on it."
            ));
        }
        if self.running {
            return Err(format!("{path} is the drive this system runs from."));
        }
        if self.bus == Bus::Inside {
            return Err(format!(
                "{path} is neither removable nor on USB. Eclipse is only written onto a stick or a USB disk, so a disk inside a computer is never erased."
            ));
        }
        if self.read_only {
            return Err(format!("{path} is read only."));
        }
        if let Some(why) = in_use {
            return Err(format!("{why}. Unmount or close it first."));
        }
        if let Some(bytes) = self.sector.filter(|&bytes| bytes != SECTOR) {
            return Err(format!(
                "{path} has sectors of {bytes} bytes, and the system partitions are made for sectors of {SECTOR}."
            ));
        }
        if self.size < needed {
            return Err(format!(
                "{path} holds {}, and Eclipse needs {}.",
                size(self.size),
                size(needed)
            ));
        }
        Ok(())
    }

    /// What a person is shown about the disk before it is erased.
    #[must_use]
    pub fn describe(&self) -> Vec<String> {
        let mut about = vec![self.path.clone()];
        about.extend(self.model.clone());
        about.push(size(self.size));
        if let Some(serial) = &self.serial {
            about.push(format!("serial {serial}"));
        }
        let volumes: Vec<String> = self
            .volumes
            .iter()
            .map(|volume| match &volume.fs {
                Some(fs) => format!("{} ({fs})", volume.name),
                None => volume.name.clone(),
            })
            .collect();
        vec![
            format!("{}.", about.join(", ")),
            if volumes.is_empty() {
                "It has no partitions.".to_string()
            } else {
                format!("It holds {}.", volumes.join(", "))
            },
        ]
    }

    /// What has to be typed back before the disk is erased: its serial, or its name when it has
    /// none.
    #[must_use]
    pub fn confirmation(&self) -> &str {
        self.serial.as_deref().unwrap_or(&self.name)
    }
}

/// Text a device reports, trimmed, when there is any.
pub(crate) fn reported(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::super::{GIB, needed};
    use super::*;

    fn stick() -> Disk {
        Disk {
            name: "disk4".into(),
            path: "/dev/disk4".into(),
            whole: true,
            running: false,
            bus: Bus::Usb,
            read_only: false,
            sector: Some(512),
            size: 61_530_439_680,
            model: Some("SanDisk Ultra Fit".into()),
            serial: None,
            volumes: vec![Volume {
                path: "/dev/disk4s1".into(),
                name: "STICK".into(),
                fs: Some("exfat".into()),
                mounted: Some("/Volumes/STICK".into()),
            }],
        }
    }

    #[test]
    fn a_stick_passes_and_is_shown_by_its_name() {
        let stick = stick();
        assert!(stick.listed());
        assert_eq!(stick.refuse(needed(GIB, None), None), Ok(()));
        assert_eq!(
            stick.describe(),
            [
                "/dev/disk4, SanDisk Ultra Fit, 57.3 GiB.",
                "It holds STICK (exfat)."
            ]
        );
        assert_eq!(stick.confirmation(), "disk4");
        assert_eq!(
            stick.mounted().as_deref(),
            Some("/dev/disk4s1 is mounted on /Volumes/STICK")
        );
    }

    #[test]
    fn an_image_with_something_mounted_is_neither_listed_nor_written() {
        let mut image = stick();
        image.bus = Bus::Image;
        assert!(!image.listed());
        let why = image
            .refuse(needed(GIB, None), image.mounted().as_deref())
            .unwrap_err();
        assert_eq!(
            why,
            "/dev/disk4s1 is mounted on /Volumes/STICK. Unmount or close it first."
        );
        image.volumes[0].mounted = None;
        assert!(image.listed());
        assert_eq!(
            image.refuse(needed(GIB, None), image.mounted().as_deref()),
            Ok(())
        );
    }

    #[test]
    fn every_rule_has_its_sentence() {
        let need = needed(GIB, None);
        let refused = |change: &dyn Fn(&mut Disk), words: &str| {
            let mut disk = stick();
            change(&mut disk);
            let why = disk.refuse(need, None).unwrap_err();
            assert!(why.contains(words), "{why}");
        };
        refused(&|disk| disk.whole = false, "is not a whole disk");
        refused(
            &|disk| disk.running = true,
            "the drive this system runs from",
        );
        refused(
            &|disk| disk.bus = Bus::Inside,
            "neither removable nor on USB",
        );
        refused(&|disk| disk.read_only = true, "read only");
        refused(
            &|disk| disk.sector = Some(4096),
            "has sectors of 4096 bytes",
        );
        refused(
            &|disk| disk.size = 16 * GIB,
            "holds 16.0 GiB, and Eclipse needs 21.1 GiB",
        );
        let mut card = stick();
        card.bus = Bus::Removable;
        card.sector = None;
        card.serial = Some("0x1234".into());
        assert_eq!(card.refuse(need, None), Ok(()));
        assert_eq!(card.confirmation(), "0x1234");
    }

    #[test]
    fn reported_text_is_trimmed() {
        assert_eq!(
            reported(Some("  QEMU HARDDISK   ")).as_deref(),
            Some("QEMU HARDDISK")
        );
        assert_eq!(reported(Some("   ")), None);
        assert_eq!(reported(None), None);
    }
}
