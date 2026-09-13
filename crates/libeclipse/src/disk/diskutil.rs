//! What macOS's diskutil says about disks: `diskutil list -plist` for the disks and what is on them,
//! `diskutil info -plist` for how each one is attached, and the same for `/` for the disks the system
//! runs from. diskutil has no serial numbers, so a Mac's disks are typed back by name.

use super::guard::{Bus, Disk, Volume, reported};
use super::plist::{Value, read_plist};

/// The disks in what `diskutil list -plist` printed, each with what `info` returns for it, which is
/// `diskutil info -plist <disk>`. `root` is what `diskutil info -plist /` printed. A synthesized disk,
/// an APFS container over a partition or a disk image, is no whole disk here.
///
/// # Errors
///
/// When diskutil printed something other than a property list of disks, or `info` fails.
pub fn read_diskutil(
    list: &str,
    root: &str,
    info: &mut impl FnMut(&str) -> Result<String, String>,
) -> Result<Vec<Disk>, String> {
    let list = plist(list)?;
    let running = running(&plist(root)?);
    let entries = list.items("AllDisksAndPartitions");
    entries
        .iter()
        .map(|entry| {
            let name = entry
                .text("DeviceIdentifier")
                .ok_or("diskutil listed a disk without a name.")?;
            let mut disk = disk_from(name, &plist(&info(name)?)?, &running);
            if !entry.items("APFSPhysicalStores").is_empty() {
                disk.whole = false;
            }
            disk.volumes = volumes(entries, entry, name);
            Ok(disk)
        })
        .collect()
}

/// A disk or a partition from what `diskutil info -plist <name>` printed alone, for a name that is no
/// disk in `diskutil list`. `root` is what `diskutil info -plist /` printed.
///
/// # Errors
///
/// When diskutil printed something other than a property list.
pub fn read_diskutil_info(name: &str, info: &str, root: &str) -> Result<Disk, String> {
    Ok(disk_from(name, &plist(info)?, &running(&plist(root)?)))
}

fn plist(text: &str) -> Result<Value, String> {
    read_plist(text).map_err(|why| format!("diskutil printed something Eclipse cannot read. {why}"))
}

/// The whole disks the system runs from: the one `/` is on, and the ones under its APFS container.
fn running(root: &Value) -> Vec<String> {
    let mut disks: Vec<String> = root
        .text("ParentWholeDisk")
        .map(whole_disk)
        .into_iter()
        .collect();
    disks.extend(
        root.items("APFSPhysicalStores")
            .iter()
            .filter_map(|store| store.text("APFSPhysicalStore"))
            .map(whole_disk),
    );
    disks
}

/// The name of the whole disk a device is on: `disk0` for `disk0s2`, `disk3` for `disk3s1s1`.
fn whole_disk(name: &str) -> String {
    let digits = name.strip_prefix("disk").map_or(0, |rest| {
        rest.bytes().take_while(u8::is_ascii_digit).count()
    });
    if digits == 0 {
        return name.to_string();
    }
    name[..4 + digits].to_string()
}

fn disk_from(name: &str, about: &Value, running: &[String]) -> Disk {
    let bus = match about.text("BusProtocol") {
        Some("Disk Image") => Bus::Image,
        Some("USB") => Bus::Usb,
        _ if about.flag("RemovableMedia") => Bus::Removable,
        _ => Bus::Inside,
    };
    Disk {
        name: name.to_string(),
        path: about
            .text("DeviceNode")
            .map_or_else(|| format!("/dev/{name}"), str::to_string),
        whole: about.flag("WholeDisk"),
        running: running.iter().any(|disk| disk == name),
        bus,
        read_only: !about.flag("WritableMedia"),
        sector: about.number("DeviceBlockSize"),
        size: about
            .number("Size")
            .or_else(|| about.number("TotalSize"))
            .unwrap_or(0),
        model: reported(about.text("MediaName")),
        serial: None,
        volumes: Vec::new(),
    }
}

/// What is on the disk `name`, whose entry in `diskutil list` is `entry`: a file system on the disk
/// itself, its partitions, and the volumes of the APFS containers that are stored on it.
fn volumes(entries: &[Value], entry: &Value, name: &str) -> Vec<Volume> {
    let volume = |item: &Value| {
        item.text("DeviceIdentifier").map(|id| Volume {
            path: format!("/dev/{id}"),
            name: item.text("VolumeName").unwrap_or(id).to_string(),
            fs: None,
            mounted: item.text("MountPoint").map(str::to_string),
        })
    };
    let mut found: Vec<Volume> = Vec::new();
    if entry.text("MountPoint").is_some() || entry.text("VolumeName").is_some() {
        found.extend(volume(entry));
    }
    found.extend(entry.items("Partitions").iter().filter_map(volume));
    for container in entries {
        if container
            .items("APFSPhysicalStores")
            .iter()
            .filter_map(|store| store.text("DeviceIdentifier"))
            .any(|store| whole_disk(store) == name)
        {
            found.extend(container.items("APFSVolumes").iter().filter_map(volume));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::super::needed;
    use super::*;

    // what diskutil printed on GitHub's macos-latest runner (macOS 26.6.2 in a vm), with an empty 22G
    // raw file attached by hdiutil as disk7. disk3 and disk5 are the system's own asset images,
    // mounted through the APFS containers disk4 and disk6
    include!("testdata/diskutil_runner.rs");

    fn info(name: &str) -> Result<String, String> {
        Ok(match name {
            "disk0" => DISK0,
            "disk1" => DISK1,
            "disk2" => DISK2,
            "disk3" => DISK3,
            "disk4" => DISK4,
            "disk5" => DISK5,
            "disk6" => DISK6,
            "disk7" => DISK7,
            _ => return Err(format!("Could not find disk: {name}")),
        }
        .to_string())
    }

    fn runner() -> Vec<Disk> {
        read_diskutil(LIST, ROOT, &mut info).unwrap()
    }

    #[test]
    fn the_runner_s_disks_are_read_with_what_is_on_them() {
        let disks = runner();
        let names: Vec<&str> = disks.iter().map(|disk| disk.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "disk0", "disk1", "disk2", "disk3", "disk4", "disk5", "disk6", "disk7"
            ]
        );
        let listed: Vec<&str> = disks
            .iter()
            .filter(|disk| disk.listed())
            .map(|disk| disk.name.as_str())
            .collect();
        assert_eq!(listed, ["disk7"]);
        let need = needed(0, None);
        let refused = |index: usize, words: &str| {
            let disk = &disks[index];
            let why = disk.refuse(need, disk.mounted().as_deref()).unwrap_err();
            assert!(why.contains(words), "{why}");
        };
        refused(0, "/dev/disk0 is the drive this system runs from.");
        refused(1, "/dev/disk1 is not a whole disk.");
        refused(2, "/dev/disk2 is not a whole disk.");
        refused(3, "/dev/disk3 is read only.");
        refused(4, "/dev/disk4 is not a whole disk.");

        // the system's disk holds its partitions and the volumes of the containers on them
        let system = &disks[0];
        assert_eq!(system.bus, Bus::Inside);
        assert!(
            system
                .volumes
                .iter()
                .any(|volume| volume.mounted.as_deref() == Some("/"))
        );
        assert_eq!(system.volumes[1].path, "/dev/disk0s2");

        // an asset image of the system is mounted through the container over it
        assert!(
            disks[3]
                .mounted()
                .unwrap()
                .starts_with("/dev/disk4s1 is mounted on /System/Library/AssetsV2/")
        );

        let image = &disks[7];
        assert_eq!(image.bus, Bus::Image);
        assert_eq!(image.refuse(need, image.mounted().as_deref()), Ok(()));
        assert_eq!(
            image.describe(),
            ["/dev/disk7, Disk Image, 22.0 GiB.", "It has no partitions."]
        );
        assert_eq!(image.confirmation(), "disk7");
        assert_eq!(image.sector, Some(512));
    }

    #[test]
    fn a_stick_is_written_even_with_a_volume_mounted() {
        let list = r#"<plist version="1.0"><dict><key>AllDisksAndPartitions</key><array><dict>
            <key>Content</key><string>GUID_partition_scheme</string>
            <key>DeviceIdentifier</key><string>disk7</string>
            <key>Partitions</key><array>
              <dict><key>Content</key><string>EFI</string><key>DeviceIdentifier</key><string>disk7s1</string>
                <key>VolumeName</key><string>EFI</string></dict>
              <dict><key>Content</key><string>Microsoft Basic Data</string><key>DeviceIdentifier</key>
                <string>disk7s2</string><key>MountPoint</key><string>/Volumes/STICK</string>
                <key>VolumeName</key><string>STICK</string></dict>
            </array></dict></array></dict></plist>"#;
        // the runner's image, told it is on USB and what it is called
        let about = DISK7
            .replace(
                "<key>BusProtocol</key>\n\t<string>Disk Image</string>",
                "<key>BusProtocol</key>\n\t<string>USB</string>",
            )
            .replace(
                "<key>MediaName</key>\n\t<string>Disk Image</string>",
                "<key>MediaName</key>\n\t<string>SanDisk Ultra Fit</string>",
            );
        let disks = read_diskutil(list, ROOT, &mut |_| Ok(about.clone())).unwrap();
        let [stick] = disks.as_slice() else {
            panic!("{disks:?}");
        };
        assert_eq!(stick.bus, Bus::Usb);
        assert!(stick.listed());
        // macOS mounts every stick, and eclipse-flash unmounts it before it writes
        assert_eq!(stick.refuse(needed(0, None), None), Ok(()));
        assert_eq!(
            stick.mounted().as_deref(),
            Some("/dev/disk7s2 is mounted on /Volumes/STICK")
        );
        assert_eq!(
            stick.describe(),
            [
                "/dev/disk7, SanDisk Ultra Fit, 22.0 GiB.",
                "It holds EFI, STICK."
            ]
        );
    }

    #[test]
    fn a_partition_is_no_whole_disk() {
        let about = "<plist version=\"1.0\"><dict><key>DeviceNode</key><string>/dev/disk0s2</string>\
            <key>WholeDisk</key><false/><key>WritableMedia</key><true/><key>Size</key>\
            <integer>343073095680</integer></dict></plist>";
        let partition = read_diskutil_info("disk0s2", about, ROOT).unwrap();
        assert!(
            partition
                .refuse(needed(0, None), None)
                .unwrap_err()
                .starts_with("/dev/disk0s2 is not a whole disk.")
        );
        assert!(read_diskutil(LIST, ROOT, &mut |name| Err(format!("no {name}"))).is_err());
        assert!(read_diskutil("diskutil: not a plist", ROOT, &mut info).is_err());
    }

    #[test]
    fn devices_are_on_the_whole_disk_their_name_starts_with() {
        assert_eq!(whole_disk("disk0s2"), "disk0");
        assert_eq!(whole_disk("disk3s1s1"), "disk3");
        assert_eq!(whole_disk("disk12"), "disk12");
        assert_eq!(whole_disk("disk"), "disk");
        assert_eq!(whole_disk("sda"), "sda");
    }
}
