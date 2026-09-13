//! A Mac's disks, from diskutil, and writing onto one: what is mounted from it unmounted, its raw
//! device `/dev/rdiskN` opened, and the disk ejected after.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::process::Command;

use libeclipse::disk::run::{Drive, tool};
use libeclipse::disk::{Disk, read_diskutil, read_diskutil_info};

use crate::direct::System;

/// A Mac's raw disk, `/dev/rdiskN`. Rust syncs a file on macOS with `F_FULLFSYNC`, which a raw disk
/// refuses as an inappropriate ioctl; a plain fsync reaches it.
pub struct Raw(File);

impl Read for Raw {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Raw {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Seek for Raw {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.0.seek(to)
    }
}

impl Drive for Raw {
    fn sync(&mut self) -> io::Result<()> {
        rustix::fs::fsync(&self.0).map_err(io::Error::from)
    }
}

/// The Mac eclipse-flash runs on.
pub struct Mac {
    root: bool,
    disks: Vec<Disk>,
    /// What `diskutil info -plist /` printed.
    system: String,
}

impl Mac {
    /// Asks diskutil for the Mac's disks.
    pub fn query() -> Result<Mac, String> {
        let list = diskutil(&["list", "-plist"])?;
        let system = diskutil(&["info", "-plist", "/"])?;
        let disks = read_diskutil(&list, &system, &mut |name| {
            diskutil(&["info", "-plist", name])
        })?;
        Ok(Mac {
            root: rustix::process::geteuid().is_root(),
            disks,
            system,
        })
    }
}

fn diskutil(args: &[&str]) -> Result<String, String> {
    tool(Command::new("diskutil").args(args))
}

impl System for Mac {
    type Opened = Raw;

    fn rights(&self) -> Option<&'static str> {
        (!self.root).then_some(
            "Writing a drive erases a disk, so it needs root. Run sudo eclipse-flash write <image> <disk>.",
        )
    }

    fn disks(&self) -> &[Disk] {
        &self.disks
    }

    fn disk(&self, target: &str) -> Option<Result<Disk, String>> {
        let device = target.strip_prefix("/dev/")?;
        let name = device
            .strip_prefix('r')
            .filter(|name| name.starts_with("disk"))
            .unwrap_or(device);
        if !name.starts_with("disk") {
            return None;
        }
        Some(match self.disks.iter().find(|disk| disk.name == name) {
            Some(disk) => Ok(disk.clone()),
            None => diskutil(&["info", "-plist", name])
                .map_err(|_| format!("There is no disk {target}."))
                .and_then(|info| read_diskutil_info(name, &info, &self.system)),
        })
    }

    fn open(&self, disk: &Disk) -> Result<Raw, String> {
        diskutil(&["unmountDisk", &disk.path])
            .map_err(|why| format!("Could not unmount what is on {}. {why}", disk.path))?;
        let raw = format!("/dev/r{}", disk.name);
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&raw)
            .map(Raw)
            .map_err(|e| format!("Could not open {raw}: {e}"))
    }

    fn finish(&self, disk: &Disk) {
        let _ = diskutil(&["eject", &disk.path]);
    }
}
