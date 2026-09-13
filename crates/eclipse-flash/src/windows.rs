//! A Windows computer's disks, from PowerShell's Get-Disk, and writing onto one: its partitions
//! removed with Clear-Disk, `\\.\PhysicalDriveN` opened, and Windows told to read the new table after.
//! Nothing here needs an ioctl: a disk with no partitions has no volume for Windows to protect.

use std::fs::{File, OpenOptions};
use std::process::Command;

use libeclipse::disk::{Disk, GET_DISK, read_get_disk};

use crate::direct::System;

/// The Windows computer eclipse-flash runs on.
pub struct Windows {
    administrator: bool,
    disks: Vec<Disk>,
}

impl Windows {
    /// Asks PowerShell for the disks, and whether this runs as an administrator.
    pub fn query() -> Result<Windows, String> {
        let (administrator, disks) = read_get_disk(&powershell(GET_DISK)?)?;
        Ok(Windows {
            administrator,
            disks,
        })
    }
}

/// Runs one line of PowerShell and returns what it printed.
fn powershell(line: &str) -> Result<String, String> {
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", line])
        .output()
        .map_err(|e| format!("Could not run PowerShell: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "PowerShell failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// The number of a disk by its name, 2 for `PhysicalDrive2`.
fn number(name: &str) -> Option<u32> {
    name.strip_prefix("PhysicalDrive")?.parse().ok()
}

impl System for Windows {
    type Opened = File;

    fn rights(&self) -> Option<&'static str> {
        (!self.administrator).then_some(
            "Writing a drive erases a disk, so it needs an administrator. Open the terminal with Run as administrator and run eclipse-flash write again.",
        )
    }

    fn disks(&self) -> &[Disk] {
        &self.disks
    }

    fn disk(&self, target: &str) -> Option<Result<Disk, String>> {
        let lower = target.to_ascii_lowercase();
        let digits = lower
            .strip_prefix(r"\\.\")
            .unwrap_or(lower.as_str())
            .strip_prefix("physicaldrive")?;
        Some(
            digits
                .parse::<u32>()
                .ok()
                .and_then(|wanted| {
                    self.disks
                        .iter()
                        .find(|disk| number(&disk.name) == Some(wanted))
                })
                .cloned()
                .ok_or_else(|| format!("There is no disk {target}.")),
        )
    }

    fn open(&self, disk: &Disk) -> Result<File, String> {
        let number =
            number(&disk.name).ok_or_else(|| format!("There is no disk {}.", disk.path))?;
        powershell(&format!(
            "$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'; \
             if ([string](Get-Disk -Number {number}).PartitionStyle -ne 'RAW') \
             {{ Clear-Disk -Number {number} -RemoveData -RemoveOEM -Confirm:$false }}"
        ))
        .map_err(|why| format!("Could not remove the partitions on {}. {why}", disk.path))?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&disk.path)
            .map_err(|e| format!("Could not open {}: {e}", disk.path))
    }

    fn finish(&self, disk: &Disk) {
        if let Some(number) = number(&disk.name) {
            let _ = powershell(&format!("Update-Disk -Number {number}"));
        }
    }
}
