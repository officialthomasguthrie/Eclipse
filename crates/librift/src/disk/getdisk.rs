//! What Windows says about its disks: the answer to the PowerShell query rift-flash runs, Get-Disk
//! and Get-Partition with each partition's volume, as JSON.

use serde::Deserialize;

use super::guard::{Bus, Disk, Volume, reported};

/// The PowerShell query, on one line and without double quotes, so it goes through as one argument
/// of `powershell -Command`: whether it runs as an administrator, the disks, and the partitions with
/// their volumes. Its output is UTF-8.
pub const GET_DISK: &str = concat!(
    "$ErrorActionPreference = 'Stop'; ",
    "$ProgressPreference = 'SilentlyContinue'; ",
    "[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false; ",
    "$me = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent(); ",
    "[pscustomobject]@{ ",
    "Administrator = $me.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator); ",
    "Disks = @(Get-Disk | ForEach-Object { [pscustomobject]@{ ",
    "Number = $_.Number; FriendlyName = $_.FriendlyName; SerialNumber = $_.SerialNumber; ",
    "BusType = [string]$_.BusType; Size = $_.Size; LogicalSectorSize = $_.LogicalSectorSize; ",
    "IsBoot = $_.IsBoot; IsSystem = $_.IsSystem; IsReadOnly = $_.IsReadOnly } }); ",
    "Partitions = @(Get-Partition | ForEach-Object { ",
    "$volume = $_ | Get-Volume -ErrorAction SilentlyContinue; [pscustomobject]@{ ",
    "DiskNumber = $_.DiskNumber; PartitionNumber = $_.PartitionNumber; ",
    "DriveLetter = [string]$_.DriveLetter; AccessPaths = @($_.AccessPaths); ",
    "FileSystem = $volume.FileSystem; Label = $volume.FileSystemLabel } }) ",
    "} | ConvertTo-Json -Depth 3"
);

/// A list in PowerShell's JSON, which may be a single value where a list has one item.
#[derive(Deserialize)]
#[serde(untagged)]
enum Many<T> {
    List(Vec<T>),
    One(T),
}

impl<T> Many<T> {
    fn items(&self) -> &[T] {
        match self {
            Many::List(items) => items,
            Many::One(item) => std::slice::from_ref(item),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Answer {
    administrator: bool,
    disks: Option<Many<Found>>,
    partitions: Option<Many<Part>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Found {
    number: u32,
    friendly_name: Option<String>,
    serial_number: Option<String>,
    bus_type: Option<String>,
    size: Option<u64>,
    logical_sector_size: Option<u64>,
    is_boot: Option<bool>,
    is_system: Option<bool>,
    is_read_only: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Part {
    disk_number: u32,
    partition_number: u32,
    drive_letter: Option<String>,
    access_paths: Option<Many<Option<String>>>,
    file_system: Option<String>,
    label: Option<String>,
}

/// Whether PowerShell ran as an administrator, and the disks it found, in the order it found them.
///
/// # Errors
///
/// When PowerShell printed something other than the answer to [`GET_DISK`].
pub fn read_get_disk(json: &str) -> Result<(bool, Vec<Disk>), String> {
    let answer: Answer = serde_json::from_str(json.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("PowerShell printed something Rift cannot read: {e}"))?;
    let partitions = answer.partitions.as_ref().map_or(&[][..], Many::items);
    let disks = answer
        .disks
        .as_ref()
        .map_or(&[][..], Many::items)
        .iter()
        .map(|found| {
            let path = format!(r"\\.\PhysicalDrive{}", found.number);
            let bus = match found
                .bus_type
                .as_deref()
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("usb") => Bus::Usb,
                Some("sd" | "mmc") => Bus::Removable,
                Some("file backed virtual") => Bus::Image,
                _ => Bus::Inside,
            };
            let volumes = partitions
                .iter()
                .filter(|part| part.disk_number == found.number)
                .map(|part| volume(&path, part))
                .collect();
            Disk {
                name: format!("PhysicalDrive{}", found.number),
                whole: true,
                running: found.is_boot == Some(true) || found.is_system == Some(true),
                bus,
                read_only: found.is_read_only == Some(true),
                sector: found.logical_sector_size,
                size: found.size.unwrap_or(0),
                model: reported(found.friendly_name.as_deref()),
                serial: reported(found.serial_number.as_deref()),
                volumes,
                path,
            }
        })
        .collect();
    Ok((answer.administrator, disks))
}

/// A partition as the rules see it: mounted when it has a drive letter or a folder it is mounted
/// on. Every volume also has a `\\?\Volume{...}` path, which is not a mount.
fn volume(disk: &str, part: &Part) -> Volume {
    let letter = part
        .drive_letter
        .as_deref()
        .map(|letter| letter.trim_matches(|c: char| c == '\0' || c.is_whitespace()))
        .filter(|letter| !letter.is_empty())
        .map(|letter| format!("{letter}:\\"));
    let folder = || {
        part.access_paths
            .as_ref()
            .map_or(&[][..], Many::items)
            .iter()
            .flatten()
            .find(|path| !path.starts_with(r"\\?\"))
            .cloned()
    };
    Volume {
        path: format!("Partition {} of {disk}", part.partition_number),
        name: reported(part.label.as_deref())
            .unwrap_or_else(|| format!("partition {}", part.partition_number)),
        fs: reported(part.file_system.as_deref()),
        mounted: letter.or_else(folder),
    }
}

#[cfg(test)]
mod tests {
    use super::super::needed;
    use super::*;

    // what the query printed on GitHub's windows-latest runner, as an administrator, with an empty
    // 22G VHDX attached through diskpart: the image first, then the runner's own two disks
    include!("testdata/get_disk_runner.rs");

    /// A USB stick with an exFAT volume on E:, in the shape PowerShell gives a list of one.
    const STICK: &str = r#"{
        "Administrator": false,
        "Disks": {"Number": 3, "FriendlyName": "SanDisk Ultra Fit", "SerialNumber": "4C530001230717117401  ",
                  "BusType": "USB", "Size": 61530439680, "LogicalSectorSize": 512, "IsBoot": false,
                  "IsSystem": false, "IsReadOnly": false},
        "Partitions": {"DiskNumber": 3, "PartitionNumber": 1, "DriveLetter": "E",
                       "AccessPaths": ["E:\\", "\\\\?\\Volume{8f3a2c1e-0000-0000-0000-100000000000}\\"],
                       "FileSystem": "exFAT", "Label": "STICK"}
    }"#;

    #[test]
    fn the_runner_s_disks_are_read_with_their_partitions() {
        let (administrator, disks) = read_get_disk(RUNNER).unwrap();
        assert!(administrator);
        let names: Vec<&str> = disks.iter().map(|disk| disk.name.as_str()).collect();
        assert_eq!(
            names,
            ["PhysicalDrive2", "PhysicalDrive0", "PhysicalDrive1"]
        );
        let need = needed(0, None);

        let image = &disks[0];
        assert_eq!(image.bus, Bus::Image);
        assert_eq!(image.path, r"\\.\PhysicalDrive2");
        assert_eq!(image.size, 23_622_320_128);
        assert!(image.listed());
        assert_eq!(image.refuse(need, image.mounted().as_deref()), Ok(()));
        assert_eq!(
            image.describe(),
            [
                r"\\.\PhysicalDrive2, Msft Virtual Disk, 22.0 GiB.",
                "It has no partitions."
            ]
        );
        assert_eq!(image.confirmation(), "PhysicalDrive2");

        let system = &disks[1];
        assert!(system.running && !system.listed());
        assert_eq!(
            system.refuse(need, None),
            Err(r"\\.\PhysicalDrive0 is the drive this system runs from.".into())
        );
        assert_eq!(
            system.describe()[1],
            "It holds partition 1, Recovery (NTFS), partition 3 (FAT32), Windows (NTFS)."
        );
        assert_eq!(
            system.mounted().as_deref(),
            Some(r"Partition 4 of \\.\PhysicalDrive0 is mounted on C:\")
        );

        let temporary = &disks[2];
        assert_eq!(temporary.bus, Bus::Inside);
        assert!(!temporary.listed());
        assert!(
            temporary
                .refuse(need, None)
                .unwrap_err()
                .contains("neither removable nor on USB")
        );
        assert_eq!(
            temporary.mounted().as_deref(),
            Some(r"Partition 1 of \\.\PhysicalDrive1 is mounted on D:\")
        );
    }

    #[test]
    fn a_stick_is_written_even_with_a_drive_letter() {
        let (administrator, disks) = read_get_disk(&format!("\u{feff}{STICK}")).unwrap();
        assert!(!administrator);
        let [stick] = disks.as_slice() else {
            panic!("{disks:?}");
        };
        assert_eq!(stick.bus, Bus::Usb);
        assert!(stick.listed());
        // Windows gives its sticks a letter, and rift-flash clears the disk before it writes
        assert_eq!(stick.refuse(needed(0, None), None), Ok(()));
        assert_eq!(
            stick.mounted().as_deref(),
            Some(r"Partition 1 of \\.\PhysicalDrive3 is mounted on E:\")
        );
        assert_eq!(
            stick.describe(),
            [
                r"\\.\PhysicalDrive3, SanDisk Ultra Fit, 57.3 GiB, serial 4C530001230717117401.",
                "It holds STICK (exFAT)."
            ]
        );
        assert_eq!(stick.confirmation(), "4C530001230717117401");

        let card = read_get_disk(&STICK.replace("\"USB\"", "\"SD\""))
            .unwrap()
            .1;
        assert_eq!(card[0].bus, Bus::Removable);
        let locked = read_get_disk(&STICK.replace("\"IsReadOnly\": false", "\"IsReadOnly\": true"))
            .unwrap()
            .1;
        assert!(
            locked[0]
                .refuse(needed(0, None), None)
                .unwrap_err()
                .ends_with("is read only.")
        );
        let bare =
            read_get_disk(r#"{"Administrator": true, "Disks": [], "Partitions": []}"#).unwrap();
        assert_eq!(bare, (true, Vec::new()));
        assert!(read_get_disk("Get-Disk : Access denied").is_err());
    }
}
