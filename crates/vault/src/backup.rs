//! Backups: rustic copies home into an encrypted repository in a folder on another disk.
//!
//! A backup reads a read-only snapshot of home taken for it and deleted after, never live home.
//! The target is the uuid of a file system and a folder on it, so Vault mounts the disk itself
//! whenever it is plugged in. The password is in a file only root reads. A restore goes the way
//! one from a snapshot does: rustic writes the file, and the folders above it with their owners
//! and modes, into a folder only root writes, and the copy into home runs as the account that
//! asked.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::restore::{self, Account, Outcome, Problem, Source};
use crate::timeline;

/// Where home is in every backup, whatever folder rustic read it from.
const AS_PATH: &str = "/home";
/// Every backup of home has this label, so rustic finds the one before and reads only what changed.
const LABEL: &str = "home";
const TARGET_FILE: &str = "backup.toml";
const KEY_FILE: &str = "backup.key";
/// Crockford's base32 in lower case: no i, l, o or u to misread.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Numbers the mount points and staging folders of one process.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Where backups go: a folder on a file system, found by the file system's uuid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// The file system's uuid, as `/dev/disk/by-uuid` names it.
    pub disk: String,
    /// The repository's folder, from the root of that file system.
    pub folder: String,
}

impl Target {
    /// The target in the text of `backup.toml`.
    pub fn read(text: &str) -> Result<Target, String> {
        let target: Target =
            toml::from_str(text).map_err(|e| format!("The backup target is not readable: {e}"))?;
        if !plain_uuid(&target.disk) {
            return Err(format!(
                "\"{}\" in the backup target is not the uuid of a disk.",
                target.disk
            ));
        }
        let folder = Path::new(&target.folder);
        if !folder.is_absolute() || folder.components().any(|c| c == Component::ParentDir) {
            return Err(format!(
                "\"{}\" in the backup target is not a full path on the disk.",
                target.folder
            ));
        }
        Ok(target)
    }

    /// The text of `backup.toml` for this target.
    pub fn text(&self) -> Result<String, String> {
        let body =
            toml::to_string(self).map_err(|e| format!("Could not write the backup target: {e}"))?;
        Ok(format!(
            "# where vault backs up home. sudo vault target <folder> writes this\n{body}"
        ))
    }
}

/// A uuid as file systems have them: hex digits and dashes, `1b2c3d4e-...` or `ABCD-1234`.
pub fn plain_uuid(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && text.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        && text.bytes().any(|b| b.is_ascii_hexdigit())
}

/// The file system a path is on, from `findmnt --json --output UUID,FSROOT,TARGET --target`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Mount {
    /// None for a file system without one, like tmpfs.
    pub uuid: Option<String>,
    /// The folder of the file system that is mounted, `/` unless it is a subvolume or a bind.
    pub fsroot: String,
    /// Where it is mounted.
    pub target: String,
}

/// The file system in what findmnt printed.
pub fn read_findmnt(json: &str) -> Result<Mount, String> {
    #[derive(Deserialize)]
    struct Findmnt {
        filesystems: Vec<Mount>,
    }
    let found: Findmnt =
        serde_json::from_str(json).map_err(|e| format!("findmnt printed something else: {e}"))?;
    found
        .filesystems
        .into_iter()
        .next()
        .ok_or_else(|| "findmnt printed no file system.".to_string())
}

/// Where `folder`, a full path without links, is on its file system, from that file system's root.
pub fn folder_on_disk(mount: &Mount, folder: &Path) -> Option<String> {
    let rest = folder.strip_prefix(&mount.target).ok()?;
    let path: PathBuf = Path::new(&mount.fsroot)
        .components()
        .chain(rest.components())
        .collect();
    let plain = path.is_absolute()
        && path
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)));
    plain.then(|| path.to_str().map(ToString::to_string))?
}

/// The password for 16 random bytes: 25 letters and digits in groups of five, 125 bits.
pub fn password(random: [u8; 16]) -> String {
    let bits = u128::from_be_bytes(random);
    let mut text = String::with_capacity(29);
    for index in 0..25 {
        if index > 0 && index % 5 == 0 {
            text.push('-');
        }
        let symbol = u8::try_from((bits >> (123 - 5 * index)) & 31).unwrap_or_default();
        text.push(char::from(ALPHABET[usize::from(symbol)]));
    }
    text
}

/// A backup in a repository: its id and when it was made, in seconds since the epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    pub id: String,
    pub time: i64,
}

impl Made {
    /// The first 8 digits of the id, which is what people see and type.
    pub fn short(&self) -> &str {
        self.id.get(..8).unwrap_or(&self.id)
    }
}

#[derive(Deserialize)]
struct Snapshot {
    id: String,
    time: String,
}

impl Snapshot {
    fn made(self) -> Result<Made, String> {
        let time = seconds(&self.time)
            .ok_or_else(|| format!("rustic gave backup {} the time \"{}\".", self.id, self.time))?;
        Ok(Made { id: self.id, time })
    }
}

/// The backup `rustic backup --json` made.
pub fn read_backup(json: &str) -> Result<Made, String> {
    serde_json::from_str::<Snapshot>(json)
        .map_err(|e| format!("rustic did not say which backup it made: {e}"))?
        .made()
}

/// The backups `rustic snapshots --json` listed, oldest first.
pub fn read_list(json: &str) -> Result<Vec<Made>, String> {
    #[derive(Deserialize)]
    struct Group {
        snapshots: Vec<Snapshot>,
    }
    let groups: Vec<Group> = serde_json::from_str(json)
        .map_err(|e| format!("rustic listed the backups in a way Vault cannot read: {e}"))?;
    let mut made = groups
        .into_iter()
        .flat_map(|group| group.snapshots)
        .map(Snapshot::made)
        .collect::<Result<Vec<_>, _>>()?;
    made.sort_by(|a, b| (a.time, &a.id).cmp(&(b.time, &b.id)));
    Ok(made)
}

/// Seconds since the epoch of an RFC 3339 time with any fraction and offset, the way rustic
/// writes them: `2026-09-12T19:48:06.984255+12:00`.
pub fn seconds(time: &str) -> Option<i64> {
    let local = timeline::parse(&format!("{}Z", time.get(..19)?))?;
    let mut rest = time.get(19..)?;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        rest = fraction.get(digits..)?;
    }
    let digit = |b: u8| b.is_ascii_digit().then(|| i64::from(b - b'0'));
    let offset = match *rest.as_bytes() {
        [b'Z'] => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let minutes = (digit(h1)? * 10 + digit(h2)?) * 60 + digit(m1)? * 10 + digit(m2)?;
            if sign == b'+' { minutes } else { -minutes }
        }
        _ => return None,
    };
    Some(local - offset * 60)
}

/// Whether `text` names a backup: `latest`, or 8 to 64 lower case hex digits of its id.
pub fn is_id(text: &str) -> bool {
    text == "latest"
        || ((8..=64).contains(&text.len())
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// The options every rustic run starts with.
fn base(repository: &Path, key: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["--repository".into(), repository.into()];
    args.extend(["--password-file".into(), key.into()]);
    args.extend(["--no-cache", "--no-progress", "--log-level", "warn"].map(OsString::from));
    args
}

pub fn init_args(repository: &Path, key: &Path) -> Vec<OsString> {
    let mut args = base(repository, key);
    args.push("init".into());
    args
}

pub fn backup_args(repository: &Path, key: &Path, source: &Path) -> Vec<OsString> {
    let mut args = base(repository, key);
    args.extend(["backup", "--json", "--label", LABEL, "--as-path", AS_PATH].map(OsString::from));
    args.push(source.into());
    args
}

pub fn list_args(repository: &Path, key: &Path) -> Vec<OsString> {
    let mut args = base(repository, key);
    // --json lists every backup. rustic refuses --all next to it, that is for its table
    args.extend(["snapshots", "--json", "--filter-label", LABEL].map(OsString::from));
    args
}

/// Restores `rest`, a path under home, from backup `id` into `stage`, with the folders above it.
pub fn restore_args(
    repository: &Path,
    key: &Path,
    id: &str,
    rest: &Path,
    stage: &Path,
) -> Vec<OsString> {
    let mut args = base(repository, key);
    args.extend(["restore", "--numeric-id"].map(OsString::from));
    for glob in globs(rest) {
        args.extend(["--glob".into(), glob.into()]);
    }
    args.push(format!("{id}:{AS_PATH}").into());
    args.push(stage.into());
    args
}

/// One anchored glob for every folder on the way to `rest` and one for `rest` itself. A glob only
/// restores what it matches, so without the folders' own they would come back with default modes.
pub fn globs(rest: &Path) -> Vec<String> {
    let mut pattern = String::new();
    rest.components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_string_lossy()),
            _ => None,
        })
        .map(|name| {
            pattern.push('/');
            for c in name.chars() {
                if "\\*?[]{}! ".contains(c) {
                    pattern.push('\\');
                }
                pattern.push(c);
            }
            pattern.clone()
        })
        .collect()
}

/// The sentence in what rustic printed on stderr: the paragraph after `Message:`, or its last line.
pub fn message(stderr: &str) -> String {
    let plain = without_escapes(stderr);
    let lines: Vec<&str> = plain.lines().map(str::trim).collect();
    let paragraph: Vec<&str> = lines
        .iter()
        .position(|line| *line == "Message:")
        .map(|at| {
            lines[at + 1..]
                .iter()
                .skip_while(|line| line.is_empty())
                .take_while(|line| !line.is_empty())
                .copied()
                .collect()
        })
        .unwrap_or_default();
    if !paragraph.is_empty() {
        return paragraph.join(" ");
    }
    // a mistake in the arguments is one error line, then usage and a line about --help
    lines
        .iter()
        .find(|line| line.starts_with("error:"))
        .or_else(|| {
            lines
                .iter()
                .rev()
                .find(|line| !line.is_empty() && !line.starts_with("[INFO]"))
        })
        .map_or_else(
            || "rustic stopped without saying why.".to_string(),
            |line| (*line).to_string(),
        )
}

fn without_escapes(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            for inside in chars.by_ref() {
                if inside.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    plain
}

/// Where backups come from and where Vault keeps what it needs for them.
#[derive(Debug, Clone)]
pub struct Backups {
    /// The subvolume that is backed up, `/persist/@home`.
    pub subvolume: PathBuf,
    /// Where the snapshot a backup reads is taken, `/persist/@snapshots/backup`.
    pub snapshots: PathBuf,
    /// The target and the password, `/var/lib/eclipse/vault`.
    pub state: PathBuf,
    /// Where a restore puts the file before it is copied into home, `/var/cache/vault`.
    pub cache: PathBuf,
    /// Where the disk is mounted while Vault uses it, `/run/vault`.
    pub run: PathBuf,
    /// Where disks are found by uuid, `/dev/disk/by-uuid`.
    pub devices: PathBuf,
}

/// The disk, mounted until this is dropped.
struct Disk {
    path: PathBuf,
}

impl Drop for Disk {
    fn drop(&mut self) {
        let _ = Command::new("umount").arg(&self.path).output();
        let _ = fs::remove_dir(&self.path);
    }
}

impl Backups {
    fn key(&self) -> PathBuf {
        self.state.join(KEY_FILE)
    }

    fn target(&self) -> Result<Target, String> {
        match fs::read_to_string(self.state.join(TARGET_FILE)) {
            Ok(text) => Target::read(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(
                "There is no backup disk yet. Choose a folder on one with sudo vault target <folder>."
                    .into(),
            ),
            Err(e) => Err(format!("Could not read the backup target: {e}")),
        }
    }

    fn mount(&self, uuid: &str) -> Result<Disk, String> {
        let device = self.devices.join(uuid);
        if !device.exists() {
            return Err("The backup disk is not plugged in.".into());
        }
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = self
            .run
            .join(format!("disk-{}-{number}", std::process::id()));
        fs::create_dir_all(&path).map_err(|e| format!("Could not make {}: {e}", path.display()))?;
        tool(Command::new("mount").arg(&device).arg(&path)).inspect_err(|_| {
            let _ = fs::remove_dir(&path);
        })?;
        Ok(Disk { path })
    }

    /// Mounts the target's disk and returns it with the repository in it and the password file.
    fn open(&self) -> Result<(Disk, PathBuf, PathBuf), String> {
        let target = self.target()?;
        let key = self.key();
        if !key.is_file() {
            return Err(format!(
                "The password of the backups is missing from {}.",
                self.state.display()
            ));
        }
        let disk = self.mount(&target.disk)?;
        let repository = disk.path.join(target.folder.trim_start_matches('/'));
        if !repository.join("config").is_file() {
            return Err(format!(
                "There are no backups in {} on the backup disk.",
                target.folder
            ));
        }
        Ok((disk, repository, key))
    }

    /// Makes `folder` the target. An empty folder gets a new repository, one that holds a
    /// repository is checked with the password. `password` asks for it when the one on this drive
    /// does not open it. Returns what to tell the person who asked.
    pub fn choose(
        &self,
        folder: &Path,
        password: impl FnOnce() -> Result<String, String>,
    ) -> Result<Vec<String>, String> {
        let shown = folder.display().to_string();
        let folder = fs::create_dir_all(folder)
            .and_then(|()| fs::canonicalize(folder))
            .map_err(|e| format!("Could not make the folder {shown}: {e}"))?;
        let shown = folder.display().to_string();
        let mount = findmnt(&folder)?;
        let Some(disk) = mount.uuid.clone().filter(|uuid| plain_uuid(uuid)) else {
            return Err(format!(
                "{shown} is not on a disk Vault can find again. Choose a folder on a disk with a file system of its own."
            ));
        };
        if findmnt(&self.subvolume)?.uuid.as_deref() == Some(disk.as_str()) {
            return Err(format!(
                "{shown} is on the same drive as home. A backup needs another disk."
            ));
        }
        let on_disk = folder_on_disk(&mount, &folder)
            .ok_or_else(|| format!("Could not work out where {shown} is on its disk."))?;

        fs::create_dir_all(&self.state)
            .and_then(|()| fs::set_permissions(&self.state, fs::Permissions::from_mode(0o700)))
            .map_err(|e| format!("Could not make {}: {e}", self.state.display()))?;
        let key = self.key();
        let fresh = self.state.join(format!(".{KEY_FILE}.new"));
        let mut said = vec![format!("Backups of home go to {shown} now.")];
        if folder.join("config").is_file() {
            // backups made before, maybe by a drive that is gone. they open with their own password
            if !(key.is_file() && rustic(&list_args(&folder, &key)).is_ok()) {
                write_private(&fresh, &password()?)?;
                if let Err(why) = rustic(&list_args(&folder, &fresh)) {
                    let _ = fs::remove_file(&fresh);
                    return Err(format!(
                        "The backups in {shown} do not open with that password: {why}"
                    ));
                }
                fs::rename(&fresh, &key)
                    .map_err(|e| format!("Could not keep the password: {e}"))?;
            }
            said.push(format!(
                "The backups already in {shown} open with this drive's password."
            ));
        } else {
            let empty = fs::read_dir(&folder)
                .map_err(|e| format!("Could not read {shown}: {e}"))?
                .next()
                .is_none();
            if !empty {
                return Err(format!(
                    "{shown} has files in it and no backups. Choose an empty folder."
                ));
            }
            let new = !key.is_file();
            if new {
                write_private(&fresh, &password_now()?)?;
                fs::rename(&fresh, &key)
                    .map_err(|e| format!("Could not keep the password: {e}"))?;
            }
            rustic(&init_args(&folder, &key))
                .map_err(|why| format!("rustic could not make a repository in {shown}: {why}"))?;
            if new {
                let text = fs::read_to_string(&key)
                    .map_err(|e| format!("Could not read the password back: {e}"))?;
                said.push(format!("The password of these backups is {}.", text.trim()));
                said.push(
                    "Write it down. A new drive needs it to open them, and nothing else can."
                        .into(),
                );
            } else {
                said.push("They use the same password as the backups before.".into());
            }
        }

        let target = Target {
            disk: disk.clone(),
            folder: on_disk,
        };
        let file = self.state.join(TARGET_FILE);
        let temporary = self.state.join(format!(".{TARGET_FILE}.new"));
        fs::write(&temporary, target.text()?)
            .and_then(|()| fs::rename(&temporary, &file))
            .map_err(|e| format!("Could not write {}: {e}", file.display()))?;
        said.push(format!(
            "Vault finds the disk by its uuid, {disk}, and mounts it when it needs it."
        ));
        Ok(said)
    }

    /// Backs up home from a snapshot taken now. Returns the backup rustic made.
    pub fn back_up(&self) -> Result<Made, String> {
        let _lock = timeline::lock(&self.snapshots)?;
        // a snapshot left by a backup that did not finish
        for name in timeline::names_in(&self.snapshots)
            .map_err(|e| format!("Could not read {}: {e}", self.snapshots.display()))?
        {
            delete_snapshot(&self.snapshots.join(name))?;
        }
        let (disk, repository, key) = self.open()?;
        let source = self.snapshots.join(timeline::name_of(timeline::now()));
        timeline::btrfs([
            "subvolume".as_ref(),
            "snapshot".as_ref(),
            "-r".as_ref(),
            self.subvolume.as_os_str(),
            source.as_os_str(),
        ])?;
        let made = rustic(&backup_args(&repository, &key, &source))
            .map_err(|why| format!("rustic could not back up home: {why}"))
            .and_then(|json| read_backup(&json));
        let deleted = delete_snapshot(&source);
        drop(disk);
        let made = made?;
        deleted?;
        Ok(made)
    }

    /// The backups on the target, oldest first.
    pub fn list(&self) -> Result<Vec<Made>, String> {
        let (disk, repository, key) = self.open()?;
        let listed = rustic(&list_args(&repository, &key))
            .map_err(|why| format!("rustic could not list the backups: {why}"))
            .and_then(|json| read_list(&json));
        drop(disk);
        listed
    }

    /// Restores `path`, a file under `home`, from backup `id` as `account`.
    pub fn restore(
        &self,
        home: &Path,
        id: &str,
        path: &str,
        replace: bool,
        account: Account,
    ) -> Result<(Outcome, PathBuf), Problem> {
        if !is_id(id) {
            return Err(Problem::Invalid(format!(
                "\"{id}\" is not the name of a backup."
            )));
        }
        let (rest, target) = restore::inside(home, path)?;
        let (disk, repository, key) = self.open().map_err(Problem::Failed)?;
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let stage = self
            .cache
            .join(format!("restore-{}-{number}", std::process::id()));
        let _ = fs::remove_dir_all(&stage);
        fs::create_dir_all(&stage)
            .map_err(|e| Problem::Failed(format!("Could not make {}: {e}", stage.display())))?;
        let restored = rustic(&restore_args(&repository, &key, id, &rest, &stage));
        drop(disk);
        let result = match restored {
            Ok(_) => restore::copy_as(
                account,
                &stage.join(&rest),
                &target,
                replace,
                Source::Backup,
            ),
            Err(why) if why.contains("No suitable id found") => {
                Err(Problem::Missing(format!("There is no backup {id}.")))
            }
            Err(why) => Err(Problem::Failed(format!(
                "rustic could not restore {path}: {why}"
            ))),
        };
        let _ = fs::remove_dir_all(&stage);
        result.map(|outcome| (outcome, target))
    }

    /// Deletes what restores left in the staging folder, when the service starts.
    pub fn clear(&self) {
        if let Ok(entries) = fs::read_dir(&self.cache) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with("restore-") {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
    }
}

fn delete_snapshot(path: &Path) -> Result<(), String> {
    timeline::btrfs(["subvolume".as_ref(), "delete".as_ref(), path.as_os_str()])
}

/// A new password from the kernel's random numbers.
fn password_now() -> Result<String, String> {
    let mut random = [0; 16];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut random))
        .map_err(|e| format!("Could not get random numbers for the password: {e}"))?;
    Ok(password(random))
}

/// Writes `text` to a new file only its owner reads.
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    let _ = fs::remove_file(path);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.sync_all()
        })
        .map_err(|e| format!("Could not write {}: {e}", path.display()))
}

fn findmnt(path: &Path) -> Result<Mount, String> {
    let json = tool(
        Command::new("findmnt")
            .args(["--json", "--output", "UUID,FSROOT,TARGET", "--target"])
            .arg(path),
    )?;
    read_findmnt(&json)
}

/// Runs rustic and returns what it printed on stdout, or its sentence when it failed.
fn rustic(args: &[OsString]) -> Result<String, String> {
    let mut command = Command::new("rustic");
    command.args(args).current_dir("/");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("RUSTIC_") {
            command.env_remove(name);
        }
    }
    let output = command
        .output()
        .map_err(|e| format!("Could not run rustic: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(message(&String::from_utf8_lossy(&output.stderr)))
    }
}

/// Runs a program that has to succeed and returns what it printed.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_reads_back_what_it_writes() {
        let target = Target {
            disk: "5f0e1c7a-8d2b-4c1e-9a3f-6b7d8e9f0a1b".into(),
            folder: "/Eclipse backups".into(),
        };
        let text = target.text().unwrap();
        assert!(text.starts_with("# where vault backs up home"), "{text}");
        assert_eq!(Target::read(&text), Ok(target));
        assert!(Target::read("disk = \"ABCD-1234\"\nfolder = \"/\"\n").is_ok());
    }

    #[test]
    fn a_target_with_a_strange_disk_or_folder_is_refused() {
        for text in [
            "disk = \"../../sda\"\nfolder = \"/Eclipse\"\n",
            "disk = \"\"\nfolder = \"/Eclipse\"\n",
            "disk = \"----\"\nfolder = \"/Eclipse\"\n",
            "disk = \"ABCD-1234\"\nfolder = \"Eclipse\"\n",
            "disk = \"ABCD-1234\"\nfolder = \"/a/../../b\"\n",
            "disk = \"ABCD-1234\"\n",
        ] {
            assert!(Target::read(text).is_err(), "{text}");
        }
    }

    #[test]
    fn findmnt_says_where_a_folder_is_on_its_disk() {
        let json = r#"{
           "filesystems": [
              {"uuid": "5f0e1c7a-8d2b-4c1e-9a3f-6b7d8e9f0a1b", "fsroot": "/", "target": "/run/media/eclipse/Backup"}
           ]
        }"#;
        let mount = read_findmnt(json).unwrap();
        assert_eq!(
            mount.uuid.as_deref(),
            Some("5f0e1c7a-8d2b-4c1e-9a3f-6b7d8e9f0a1b")
        );
        assert_eq!(
            folder_on_disk(&mount, Path::new("/run/media/eclipse/Backup/Eclipse")).as_deref(),
            Some("/Eclipse")
        );
        assert_eq!(
            folder_on_disk(&mount, Path::new("/run/media/eclipse/Backup")).as_deref(),
            Some("/")
        );
        assert_eq!(folder_on_disk(&mount, Path::new("/home/eclipse")), None);

        // a btrfs subvolume, or a bind mount, starts further down its file system
        let subvolume = Mount {
            uuid: Some("1234".into()),
            fsroot: "/@backups".into(),
            target: "/mnt".into(),
        };
        assert_eq!(
            folder_on_disk(&subvolume, Path::new("/mnt/Eclipse")).as_deref(),
            Some("/@backups/Eclipse")
        );

        let tmpfs =
            read_findmnt(r#"{"filesystems": [{"uuid": null, "fsroot": "/", "target": "/tmp"}]}"#)
                .unwrap();
        assert_eq!(tmpfs.uuid, None);
        assert!(read_findmnt(r#"{"filesystems": []}"#).is_err());
        assert!(read_findmnt("findmnt: can't read").is_err());
    }

    #[test]
    fn a_password_is_25_symbols_in_groups_of_five() {
        let text = password([0xa5; 16]);
        assert_eq!(text.len(), 29);
        let groups: Vec<&str> = text.split('-').collect();
        assert_eq!(groups.len(), 5);
        assert!(groups.iter().all(|group| group.len() == 5));
        assert!(
            text.bytes().all(|b| b == b'-' || ALPHABET.contains(&b)),
            "{text}"
        );
        assert_eq!(password([0; 16]), "00000-00000-00000-00000-00000");
        assert_eq!(password([0xff; 16]), "zzzzz-zzzzz-zzzzz-zzzzz-zzzzz");
        // the first symbol is the top five bits
        let mut random = [0; 16];
        random[0] = 0b0000_1000;
        assert_eq!(password(random), "10000-00000-00000-00000-00000");
        let mut other = [0xa5; 16];
        other[15] = 0x5a;
        assert_ne!(password(other), text);
    }

    #[test]
    fn rustic_gets_the_repository_the_password_and_nothing_from_the_environment() {
        let (repository, key) = (
            Path::new("/run/vault/disk-1-0/Eclipse"),
            Path::new("/var/lib/eclipse/vault/backup.key"),
        );
        let start = [
            "--repository",
            "/run/vault/disk-1-0/Eclipse",
            "--password-file",
            "/var/lib/eclipse/vault/backup.key",
            "--no-cache",
            "--no-progress",
            "--log-level",
            "warn",
        ];
        let with = |rest: &[&str]| -> Vec<OsString> {
            start.iter().chain(rest).map(OsString::from).collect()
        };
        assert_eq!(init_args(repository, key), with(&["init"]));
        assert_eq!(
            backup_args(
                repository,
                key,
                Path::new("/persist/@snapshots/backup/2026-09-12T14:00:03Z")
            ),
            with(&[
                "backup",
                "--json",
                "--label",
                "home",
                "--as-path",
                "/home",
                "/persist/@snapshots/backup/2026-09-12T14:00:03Z"
            ])
        );
        assert_eq!(
            list_args(repository, key),
            with(&["snapshots", "--json", "--filter-label", "home"])
        );
        assert_eq!(
            restore_args(
                repository,
                key,
                "e863e83c",
                Path::new("eclipse/backup/letter.txt"),
                Path::new("/var/cache/vault/restore-1-2")
            ),
            with(&[
                "restore",
                "--numeric-id",
                "--glob",
                "/eclipse",
                "--glob",
                "/eclipse/backup",
                "--glob",
                "/eclipse/backup/letter.txt",
                "e863e83c:/home",
                "/var/cache/vault/restore-1-2"
            ])
        );
    }

    #[test]
    fn globs_match_names_with_glob_characters_literally() {
        assert_eq!(
            globs(Path::new("eclipse/odd [1]/a*b?.txt")),
            [
                "/eclipse",
                "/eclipse/odd\\ \\[1\\]",
                "/eclipse/odd\\ \\[1\\]/a\\*b\\?.txt"
            ]
        );
        assert_eq!(
            globs(Path::new("eclipse/{x,y}!")),
            ["/eclipse", "/eclipse/\\{x,y\\}\\!"]
        );
    }

    #[test]
    fn times_come_back_in_utc() {
        assert_eq!(
            seconds("2026-09-12T14:00:03Z"),
            timeline::parse("2026-09-12T14:00:03Z")
        );
        assert_eq!(
            seconds("2026-09-13T02:00:03.984255+12:00"),
            timeline::parse("2026-09-12T14:00:03Z")
        );
        assert_eq!(
            seconds("2026-09-12T09:30:03.5-04:30"),
            timeline::parse("2026-09-12T14:00:03Z")
        );
        for time in [
            "",
            "2026-09-12T14:00:03",
            "2026-09-12T14:00:03.Z",
            "2026-09-12T14:00:03+1200",
            "2026-09-12T14:00:03+12:0x",
            "2026-09-12 14:00:03Z",
        ] {
            assert_eq!(seconds(time), None, "{time:?}");
        }
    }

    /// What rustic 0.11.4 printed for `backup --json`, trimmed.
    const BACKUP: &str = r#"{
  "time": "2026-09-12T19:48:06.984255+12:00",
  "program_version": "rustic 0.11.4",
  "tree": "ff9d418a06a0b04453724a5debc6f88a5e2fd9a588881c2227df18515072407e",
  "paths": ["/home"],
  "hostname": "eclipse",
  "summary": {"files_new": 3, "backup_duration": 0.024082},
  "id": "e863e83c77b4f162be953870ddaaf7ff70f92ab02e129fa01b3752e0495b489d"
}"#;

    #[test]
    fn the_backup_rustic_made_is_read_from_its_json() {
        let made = read_backup(BACKUP).unwrap();
        assert_eq!(
            made.id,
            "e863e83c77b4f162be953870ddaaf7ff70f92ab02e129fa01b3752e0495b489d"
        );
        assert_eq!(made.short(), "e863e83c");
        assert_eq!(timeline::name_of(made.time), "2026-09-12T07:48:06Z");
        assert!(read_backup("{}").is_err());
        assert!(read_backup(&BACKUP.replace("2026-09-12T19", "yesterday")).is_err());
    }

    #[test]
    fn backups_are_listed_oldest_first_across_groups() {
        let json = r#"[
  {"group_key": {"hostname": "eclipse", "label": "home", "paths": ["/home"]},
   "snapshots": [
     {"time": "2026-09-12T10:00:00+00:00", "id": "bbbbbbbb00", "paths": ["/home"]},
     {"time": "2026-09-12T08:00:00+00:00", "id": "aaaaaaaa00", "paths": ["/home"]}
   ]},
  {"group_key": {"hostname": "laptop", "label": "home", "paths": ["/home"]},
   "snapshots": [
     {"time": "2026-09-12T21:00:00+12:00", "id": "cccccccc00", "paths": ["/home"]}
   ]}
]"#;
        let listed = read_list(json).unwrap();
        let shown: Vec<(&str, String)> = listed
            .iter()
            .map(|made| (made.short(), timeline::name_of(made.time)))
            .collect();
        assert_eq!(
            shown,
            [
                ("aaaaaaaa", "2026-09-12T08:00:00Z".to_string()),
                ("cccccccc", "2026-09-12T09:00:00Z".to_string()),
                ("bbbbbbbb", "2026-09-12T10:00:00Z".to_string())
            ]
        );
        assert_eq!(read_list("[]"), Ok(Vec::new()));
        assert!(read_list("total: 0 snapshot(s)").is_err());
    }

    #[test]
    fn a_backup_is_named_by_its_id() {
        for id in [
            "latest",
            "e863e83c",
            "e863e83c77b4f162be953870ddaaf7ff70f92ab02e129fa01b3752e0495b489d",
        ] {
            assert!(is_id(id), "{id}");
        }
        for id in [
            "",
            "e863e83",
            "E863E83C",
            "e863e83c:/etc",
            "--password",
            "latest~1",
            "e863e83c77b4f162be953870ddaaf7ff70f92ab02e129fa01b3752e0495b489d0",
        ] {
            assert!(!is_id(id), "{id}");
        }
    }

    #[test]
    fn rustic_errors_come_down_to_their_message() {
        let wrong = "[INFO] using no config file, none of these exist: /etc/rustic/rustic.toml\n\
            \x1b[0m\x1b[0m\x1b[1m\x1b[31merror:\x1b[0m `rustic_core` experienced an error related to `credentials handling`.\n\
            \n\
            Message:\n\
            The password that has been entered, seems to be incorrect. No suitable key found for the given password. Please check your password and try again.\n\
            \n\
            Some additional details ...\n";
        assert_eq!(
            message(wrong),
            "The password that has been entered, seems to be incorrect. No suitable key found for the given password. Please check your password and try again."
        );
        let missing = "[INFO] getting snapshot ...\n\x1b[31merror:\x1b[0m `rustic_core` experienced an error related to `the backend`.\n\nMessage:\nNo suitable id found for `deadbeef`.\n\n\nSome additional details ...\n";
        assert_eq!(message(missing), "No suitable id found for `deadbeef`.");
        assert_eq!(
            message("[INFO] using no cache\n\x1b[31merror:\x1b[0m IO error: not a terminal\n"),
            "error: IO error: not a terminal"
        );
        let usage = "error: the argument '--json' cannot be used with '--all'\n\n\
            Usage: rustic snapshots --json --filter-label <LABEL> [ID]...\n\n\
            For more information, try '--help'.\n";
        assert_eq!(
            message(usage),
            "error: the argument '--json' cannot be used with '--all'"
        );
        assert_eq!(message(""), "rustic stopped without saying why.");
    }
}
