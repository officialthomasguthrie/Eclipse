//! The programs that write a drive, run one after another, and guards that unmount, close or detach
//! what a step left behind when a later step fails.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::{PERSIST_OPTIONS, size};

/// How much is copied at a time.
const CHUNK: usize = 4 << 20;

/// Runs a program that has to succeed, and returns what it printed.
///
/// # Errors
///
/// When the program cannot be started or fails, with what it printed on stderr.
pub fn tool(command: &mut Command) -> Result<String, String> {
    let line = format!("{command:?}");
    let output = command
        .output()
        .map_err(|e| format!("Could not run {line}: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "{line} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Runs a program that has to succeed with `input` on its stdin. The input is never part of what
/// an error says.
///
/// # Errors
///
/// When the program cannot be started, does not take its input, or fails.
pub fn feed(command: &mut Command, input: &str) -> Result<(), String> {
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

/// Whether a program of this name is in a folder on the PATH.
#[must_use]
pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
}

/// sfdisk wipes what was on the disk and on the new partitions, so nothing old is found in them.
#[must_use]
pub fn sfdisk_args(disk: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["--wipe", "always", "--wipe-partitions", "always", "--quiet"]
        .map(OsString::from)
        .into();
    args.push(disk.into());
    args
}

/// A new LUKS2 header, and with it a new volume key. The passphrase comes on stdin, as it is.
#[must_use]
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

/// Opens a LUKS volume as `/dev/mapper/<name>` with the passphrase on stdin.
#[must_use]
pub fn open_args(partition: &Path, name: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["open", "--key-file", "-"].map(OsString::from).into();
    args.extend([partition.into(), name.into()]);
    args
}

/// A file system mounted until this is dropped or unmounted.
#[derive(Debug)]
pub struct Mounted {
    path: PathBuf,
    done: bool,
}

impl Mounted {
    /// Mounts a file system of type `kind` that was just made at `path`, which is made first. Without
    /// the type mount guesses, and right after mkfs it can guess wrong.
    ///
    /// # Errors
    ///
    /// When the folder cannot be made or mount fails.
    pub fn new(
        device: &Path,
        path: PathBuf,
        kind: &str,
        options: Option<&str>,
    ) -> Result<Mounted, String> {
        fs::create_dir_all(&path).map_err(|e| format!("Could not make {}: {e}", path.display()))?;
        let mut command = Command::new("mount");
        command.args(["-t", kind]);
        if let Some(options) = options {
            command.args(["-o", options]);
        }
        tool(command.arg(device).arg(&path)).inspect_err(|_| {
            let _ = fs::remove_dir(&path);
        })?;
        Ok(Mounted { path, done: false })
    }

    /// Where it is mounted.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Unmounts it and removes the folder.
    ///
    /// # Errors
    ///
    /// When umount fails.
    pub fn unmount(mut self) -> Result<(), String> {
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
#[derive(Debug)]
pub struct Opened {
    name: String,
    done: bool,
}

impl Opened {
    /// Opens the LUKS volume on `partition` as `/dev/mapper/<name>` with `passphrase`.
    ///
    /// # Errors
    ///
    /// When cryptsetup does not open it.
    pub fn new(partition: &Path, name: &str, passphrase: &str) -> Result<Opened, String> {
        feed(
            Command::new("cryptsetup").args(open_args(partition, name)),
            passphrase,
        )?;
        Ok(Opened {
            name: name.to_string(),
            done: false,
        })
    }

    /// The device the volume opened as.
    #[must_use]
    pub fn device(&self) -> PathBuf {
        Path::new("/dev/mapper").join(&self.name)
    }

    /// Closes it.
    ///
    /// # Errors
    ///
    /// When cryptsetup does not close it.
    pub fn close(mut self) -> Result<(), String> {
        self.done = true;
        tool(Command::new("cryptsetup").args(["close", &self.name])).map(|_| ())
    }

    /// Leaves it open.
    pub fn keep(mut self) {
        self.done = true;
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

/// A loop device over part of a file, detached when this is dropped or detached.
#[derive(Debug)]
pub struct Loop {
    device: PathBuf,
    done: bool,
}

impl Loop {
    /// Attaches the `bytes` of `file` from `offset` on to a free loop device.
    ///
    /// # Errors
    ///
    /// When losetup does not attach it.
    pub fn attach(file: &Path, offset: u64, bytes: u64) -> Result<Loop, String> {
        let printed = tool(
            Command::new("losetup")
                .args(["--find", "--show", "--offset"])
                .arg(offset.to_string())
                .arg("--sizelimit")
                .arg(bytes.to_string())
                .arg(file),
        )?;
        let device = printed.trim();
        if !device.starts_with("/dev/") {
            return Err(format!("losetup printed no loop device: {device}"));
        }
        Ok(Loop {
            device: PathBuf::from(device),
            done: false,
        })
    }

    /// The loop device.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.device
    }

    /// Detaches it.
    ///
    /// # Errors
    ///
    /// When losetup does not detach it.
    pub fn detach(mut self) -> Result<(), String> {
        self.done = true;
        tool(Command::new("losetup").arg("--detach").arg(&self.device)).map(|_| ())
    }
}

impl Drop for Loop {
    fn drop(&mut self) {
        if !self.done {
            let _ = Command::new("losetup")
                .arg("--detach")
                .arg(&self.device)
                .output();
        }
    }
}

/// Persist on a new drive while it is filled: LUKS2 with a new volume key around a new btrfs,
/// mounted at its top.
#[derive(Debug)]
pub struct Persist {
    // unmounted before it is closed, fields drop in this order
    top: Mounted,
    opened: Opened,
}

impl Persist {
    /// Formats `partition` with `passphrase`, opens it as `/dev/mapper/<name>`, makes the btrfs
    /// labelled persist in it and mounts that at `mountpoint`.
    ///
    /// # Errors
    ///
    /// When any of cryptsetup, mkfs.btrfs or mount fails.
    pub fn make(
        partition: &Path,
        passphrase: &str,
        name: &str,
        mountpoint: PathBuf,
    ) -> Result<Persist, String> {
        feed(
            Command::new("cryptsetup").args(format_args(partition)),
            passphrase,
        )?;
        let opened = Opened::new(partition, name, passphrase)?;
        tool(
            Command::new("mkfs.btrfs")
                .args(["-q", "-L", "persist"])
                .arg(opened.device()),
        )?;
        let top = Mounted::new(&opened.device(), mountpoint, "btrfs", Some(PERSIST_OPTIONS))?;
        Ok(Persist { top, opened })
    }

    /// The top of the btrfs, where the subvolumes go.
    #[must_use]
    pub fn top(&self) -> &Path {
        self.top.path()
    }

    /// Makes what a new persist holds: its subvolumes, a new machine id in `@var` and the owner's
    /// home in `@home`.
    ///
    /// # Errors
    ///
    /// When btrfs fails or a file cannot be made.
    #[cfg(unix)]
    pub fn fill(&self) -> Result<(), String> {
        use std::os::unix::fs::{PermissionsExt, chown};

        let top = self.top();
        let making = |path: &Path, e: io::Error| format!("Could not make {}: {e}", path.display());
        for subvolume in super::SUBVOLUMES {
            tool(
                Command::new("btrfs")
                    .args(["subvolume", "create"])
                    .arg(top.join(subvolume)),
            )?;
        }
        let id = top.join("@var").join(super::MACHINE_ID);
        if let Some(parent) = id.parent() {
            fs::create_dir_all(parent).map_err(|e| making(parent, e))?;
        }
        fs::write(&id, super::machine_id(random()?)).map_err(|e| making(&id, e))?;
        let (owner, uid, gid) = super::OWNER;
        let home = top.join("@home").join(owner);
        fs::create_dir(&home)
            .and_then(|()| fs::set_permissions(&home, fs::Permissions::from_mode(0o755)))
            .and_then(|()| chown(&home, Some(uid), Some(gid)))
            .map_err(|e| making(&home, e))
    }

    /// Unmounts it and leaves the volume open under its name, for the boot that made it to go on
    /// with.
    ///
    /// # Errors
    ///
    /// When umount fails.
    pub fn leave_open(self) -> Result<(), String> {
        let Persist { top, opened } = self;
        top.unmount()?;
        opened.keep();
        Ok(())
    }

    /// Unmounts and closes it.
    ///
    /// # Errors
    ///
    /// When umount or cryptsetup fails.
    pub fn close(self) -> Result<(), String> {
        let Persist { top, opened } = self;
        top.unmount()?;
        opened.close()
    }
}

/// Waits for udev to make the devices of new partitions.
///
/// # Errors
///
/// When some of them are still missing after ten seconds.
pub fn settle(nodes: &[PathBuf]) -> Result<(), String> {
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

/// What a drive is written onto, opened once: a disk, a file, or a stand in for one in tests.
pub trait Drive: Read + Write + Seek {
    /// Makes sure what was written so far is on the disk, not only in a cache.
    ///
    /// # Errors
    ///
    /// When the system cannot.
    fn sync(&mut self) -> io::Result<()>;

    /// Drops what the system keeps of the drive in memory, so what is read next comes from the disk
    /// and not from a cache. Linux keeps a disk opened as a file in its page cache; a raw disk on
    /// macOS and a physical drive on Windows are read from the disk anyway.
    ///
    /// # Errors
    ///
    /// When the system cannot.
    fn uncache(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drive for File {
    fn sync(&mut self) -> io::Result<()> {
        self.sync_data()
    }
}

/// Copies `bytes` from `source`, which reads `from`, onto `to` from `offset` on, saying how far it
/// is at each tenth. With `sparse` a chunk of zeros is skipped instead of written, so `to` has to
/// read as zeros there already, the way a file cut to its size does.
///
/// # Errors
///
/// When reading or writing fails, or `source` ends early.
pub fn copy(
    source: &mut impl Read,
    from: &Path,
    to: &Path,
    offset: u64,
    bytes: u64,
    sparse: bool,
    say: &mut impl FnMut(String),
) -> Result<(), String> {
    let mut target = OpenOptions::new()
        .write(true)
        .open(to)
        .map_err(|e| format!("Could not write {}: {e}", to.display()))?;
    copy_to(
        source,
        &mut target,
        (from, to),
        (offset, bytes),
        sparse,
        say,
    )?;
    target
        .sync_all()
        .map_err(|e| format!("Could not write {}: {e}", to.display()))
}

/// Copies `bytes` from `source` onto `drive` from `offset` on, the way [`copy`] does. `from` and `to`
/// are what `source` and `drive` read and write, for what an error says.
///
/// # Errors
///
/// When reading or writing fails, or `source` ends early.
pub fn copy_to(
    source: &mut impl Read,
    drive: &mut impl Drive,
    (from, to): (&Path, &Path),
    (offset, bytes): (u64, u64),
    sparse: bool,
    say: &mut impl FnMut(String),
) -> Result<(), String> {
    let reading = |e: io::Error| format!("Could not read {}: {e}", from.display());
    let writing = |e: io::Error| format!("Could not write {}: {e}", to.display());
    drive.seek(SeekFrom::Start(offset)).map_err(writing)?;
    let mut buffer = vec![0; CHUNK];
    let mut done = 0;
    let mut tenth = 1;
    while done < bytes {
        let want = usize::try_from(bytes - done).map_or(CHUNK, |left| left.min(CHUNK));
        let chunk = &mut buffer[..want];
        source.read_exact(chunk).map_err(reading)?;
        if sparse && chunk.iter().all(|&byte| byte == 0) {
            drive
                .seek(SeekFrom::Current(i64::try_from(want).unwrap_or(i64::MAX)))
                .map_err(writing)?;
        } else {
            drive.write_all(chunk).map_err(writing)?;
        }
        done += u64::try_from(want).unwrap_or(u64::MAX);
        if tenth < 10 && done * 10 >= bytes * tenth {
            // a stick takes what is written into its cache fast and writes it out slowly
            drive.sync().map_err(writing)?;
            say(format!("Copied {} of {}.", size(done), size(bytes)));
            while tenth < 10 && done * 10 >= bytes * tenth {
                tenth += 1;
            }
        }
    }
    drive.sync().map_err(writing)
}

/// Copies a file, making the folders it goes into.
///
/// # Errors
///
/// When a folder cannot be made or the copy fails.
pub fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
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

/// 16 random bytes from the system.
///
/// # Errors
///
/// When the system has none to give.
pub fn random() -> Result<[u8; 16], String> {
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|e| format!("Could not get random numbers: {e}"))?;
    Ok(random)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_tools_get_the_disk_and_never_the_passphrase() {
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
    }

    #[test]
    fn a_copy_lands_at_its_offset_and_skips_zeros_only_when_sparse() {
        let folder = std::env::temp_dir().join(format!("libeclipse-copy-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let from = folder.join("from");
        let to = folder.join("to");
        let mut data = vec![0_u8; 3 * CHUNK];
        data[CHUNK + 5] = 7;
        data[3 * CHUNK - 1] = 9;
        fs::write(&from, &data).unwrap();

        for sparse in [false, true] {
            // what was there before the copy, where the copy writes zeros
            fs::write(&to, vec![1_u8; 4 * CHUNK]).unwrap();
            let mut said = Vec::new();
            copy(
                &mut File::open(&from).unwrap(),
                &from,
                &to,
                512,
                data.len() as u64,
                sparse,
                &mut |line| said.push(line),
            )
            .unwrap();
            let written = fs::read(&to).unwrap();
            assert_eq!(written.len(), 4 * CHUNK);
            assert_eq!(&written[..512], &[1; 512]);
            assert_eq!(written[512 + CHUNK + 5], 7);
            assert_eq!(written[512 + 3 * CHUNK - 1], 9);
            // the first chunk is all zeros: written over the ones, or skipped
            assert_eq!(written[512], u8::from(sparse));
            assert_eq!(
                said.last().map(String::as_str),
                Some("Copied 12 MiB of 12 MiB.")
            );
        }

        // a source that ends early
        fs::write(&to, vec![0_u8; 4 * CHUNK]).unwrap();
        let short = copy(
            &mut &data[..CHUNK],
            &from,
            &to,
            0,
            data.len() as u64,
            false,
            &mut |_| {},
        );
        assert!(short.unwrap_err().starts_with("Could not read"));
        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn random_bytes_differ() {
        assert_ne!(random().unwrap(), random().unwrap());
    }
}
