//! Restoring one file from a snapshot or a backup.
//!
//! Vault runs as root, but nothing here reads or writes home as root. [`restore`] checks the
//! request and [`copy_as`] runs `vault restore-file` as the account that asked, and that child does
//! [`copy_back`]. So a restore reads only what the caller could read in the snapshot and writes
//! only where the caller can write, whatever links the snapshot or home hold. A restore from a
//! backup puts the file and its folders somewhere first and copies from there the same way.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::timeline;

/// What a restore did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing was at the path, the copy is there now.
    Restored,
    /// A different file was at the path and the copy took its place.
    Replaced,
    /// The file at the path already had the same bytes. Nothing was written.
    Unchanged,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Restored => "restored",
            Outcome::Replaced => "replaced",
            Outcome::Unchanged => "unchanged",
        }
    }

    fn from_name(name: &str) -> Option<Outcome> {
        [Outcome::Restored, Outcome::Replaced, Outcome::Unchanged]
            .into_iter()
            .find(|outcome| outcome.name() == name)
    }
}

/// Why a restore did not happen, in a sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The request itself is wrong.
    Invalid(String),
    /// The snapshot, or the file in it, is not there.
    Missing(String),
    /// The file at the path differs from the copy. Only a replace overwrites it.
    Changed(String),
    /// Anything else.
    Failed(String),
}

impl Problem {
    /// The exit status `vault restore-file` reports this with.
    pub fn code(&self) -> u8 {
        match self {
            Problem::Failed(_) => 1,
            Problem::Changed(_) => 3,
            Problem::Missing(_) => 4,
            Problem::Invalid(_) => 5,
        }
    }

    pub fn sentence(&self) -> &str {
        match self {
            Problem::Invalid(s)
            | Problem::Missing(s)
            | Problem::Changed(s)
            | Problem::Failed(s) => s,
        }
    }

    fn from_code(code: Option<i32>, sentence: String) -> Problem {
        match code {
            Some(3) => Problem::Changed(sentence),
            Some(4) => Problem::Missing(sentence),
            Some(5) => Problem::Invalid(sentence),
            _ => Problem::Failed(sentence),
        }
    }
}

/// What the copy comes from, for its sentences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Snapshot,
    Backup,
}

impl Source {
    pub fn word(self) -> &'static str {
        match self {
            Source::Snapshot => "snapshot",
            Source::Backup => "backup",
        }
    }
}

/// The account that asked, as the child runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Account {
    pub uid: u32,
    pub gid: u32,
}

/// The primary group of `uid` in the text of a password file.
pub fn group_of(passwd: &str, uid: u32) -> Option<u32> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':').skip(2);
        let id: u32 = fields.next()?.parse().ok()?;
        let gid: u32 = fields.next()?.parse().ok()?;
        (id == uid).then_some(gid)
    })
}

/// The part of `path` under `home`, when `path` is a full path to something inside it with no
/// `..` or `.` in it.
pub fn under_home(home: &Path, path: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(home).ok()?;
    let plain = path.is_absolute()
        && rest.components().next().is_some()
        && rest.components().all(|c| matches!(c, Component::Normal(_)))
        && !path
            .to_string_lossy()
            .split('/')
            .any(|part| part == "." || part == "..");
    plain.then(|| rest.to_path_buf())
}

/// The part of `path` under `home` and the path itself, or why a file there cannot be restored.
pub fn inside(home: &Path, path: &str) -> Result<(PathBuf, PathBuf), Problem> {
    let target = PathBuf::from(path);
    match under_home(home, &target) {
        Some(rest) => Ok((rest, target)),
        None => Err(Problem::Invalid(format!(
            "Only files in {} can be restored. Give the full path, without . or .. in it.",
            home.display()
        ))),
    }
}

/// Restores `path`, a file under `home`, from the snapshot named `snapshot` in `snapshots`, as
/// `account`.
pub fn restore(
    snapshots: &Path,
    home: &Path,
    snapshot: &str,
    path: &str,
    replace: bool,
    account: Account,
) -> Result<(Outcome, PathBuf), Problem> {
    if timeline::parse(snapshot).is_none() {
        return Err(Problem::Invalid(format!(
            "\"{snapshot}\" is not the name of a snapshot."
        )));
    }
    let root = snapshots.join(snapshot);
    if !root.is_dir() {
        return Err(Problem::Missing(format!(
            "There is no snapshot {snapshot}."
        )));
    }
    let (rest, target) = inside(home, path)?;
    copy_as(
        account,
        &root.join(rest),
        &target,
        replace,
        Source::Snapshot,
    )
    .map(|outcome| (outcome, target))
}

/// Runs `vault restore-file` as `account` to copy `source` to `target`.
pub fn copy_as(
    account: Account,
    source: &Path,
    target: &Path,
    replace: bool,
    from: Source,
) -> Result<Outcome, Problem> {
    let program = std::env::current_exe()
        .map_err(|e| Problem::Failed(format!("Vault could not find its own program: {e}")))?;
    let mut child = Command::new(program);
    child
        .arg("restore-file")
        .arg(source)
        .arg(target)
        .uid(account.uid)
        .gid(account.gid);
    if replace {
        child.arg("--replace");
    }
    if from == Source::Backup {
        child.arg("--from-backup");
    }
    let output = child
        .output()
        .map_err(|e| Problem::Failed(format!("Vault could not start the copy: {e}")))?;
    let said = String::from_utf8_lossy(&output.stdout);
    if output.status.success() {
        Outcome::from_name(said.trim()).ok_or_else(|| {
            Problem::Failed(format!("The copy ended without saying what it did: {said}"))
        })
    } else {
        let sentence = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(Problem::from_code(output.status.code(), sentence))
    }
}

/// Copies the file `from`, in a snapshot or a restored backup, to `to`, in home. A file at `to`
/// with other bytes is only overwritten with `replace`. This runs as the account that asked for
/// the restore.
pub fn copy_back(
    from: &Path,
    to: &Path,
    replace: bool,
    source: Source,
) -> Result<Outcome, Problem> {
    let shown = to.display();
    let word = source.word();
    let metadata = match fs::symlink_metadata(from) {
        Ok(meta) if meta.is_file() => meta,
        Ok(meta) if meta.is_dir() => {
            return Err(Problem::Invalid(format!(
                "{shown} is a folder in this {word}. Only single files can be restored."
            )));
        }
        Ok(_) => {
            return Err(Problem::Invalid(format!(
                "{shown} is not a regular file in this {word}."
            )));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(Problem::Missing(format!("{shown} is not in this {word}.")));
        }
        Err(e) => {
            return Err(Problem::Failed(format!(
                "Could not read {shown} in this {word}: {e}"
            )));
        }
    };

    match fs::symlink_metadata(to) {
        Ok(meta) if meta.is_dir() => Err(Problem::Failed(format!(
            "{shown} is a folder now. Only a file can be replaced."
        ))),
        Ok(meta) => {
            let same = meta.is_file()
                && same_bytes(from, to).map_err(|e| {
                    Problem::Failed(format!("Could not compare {shown} with the {word}: {e}"))
                })?;
            if same {
                Ok(Outcome::Unchanged)
            } else if replace {
                write(from, to, &metadata)?;
                Ok(Outcome::Replaced)
            } else {
                Err(Problem::Changed(format!(
                    "{shown} has changed since this {word}."
                )))
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    Problem::Failed(format!("Could not make the folder for {shown}: {e}"))
                })?;
            }
            write(from, to, &metadata)?;
            Ok(Outcome::Restored)
        }
        Err(e) => Err(Problem::Failed(format!("Could not look at {shown}: {e}"))),
    }
}

/// Writes the copy next to `to` first and renames it over, so a copy that fails half way leaves
/// what was there whole.
fn write(from: &Path, to: &Path, source: &Metadata) -> Result<(), Problem> {
    let name = to
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let temp = to.with_file_name(format!(".{name}.restoring-{}", std::process::id()));
    copy_to(from, &temp, source)
        .and_then(|()| fs::rename(&temp, to))
        .map_err(|e| {
            let _ = fs::remove_file(&temp);
            Problem::Failed(format!("Could not write {}: {e}", to.display()))
        })
}

fn copy_to(from: &Path, temp: &Path, source: &Metadata) -> io::Result<()> {
    let mut input = File::open(from)?;
    let mut output = OpenOptions::new().write(true).create_new(true).open(temp)?;
    io::copy(&mut input, &mut output)?;
    output.set_permissions(source.permissions())?;
    output.set_modified(source.modified()?)?;
    output.sync_all()
}

fn same_bytes(snapshot: &Path, home: &Path) -> io::Result<bool> {
    if fs::metadata(snapshot)?.len() != fs::metadata(home)?.len() {
        return Ok(false);
    }
    let mut then = BufReader::new(File::open(snapshot)?);
    let mut now = BufReader::new(File::open(home)?);
    loop {
        let (old, new) = (then.fill_buf()?, now.fill_buf()?);
        if old.is_empty() || new.is_empty() {
            return Ok(old.is_empty() && new.is_empty());
        }
        let length = old.len().min(new.len());
        if old[..length] != new[..length] {
            return Ok(false);
        }
        then.consume(length);
        now.consume(length);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, SystemTime};

    const SNAPSHOT: Source = Source::Snapshot;

    /// A fresh directory under the system's temporary directory, gone again when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("vault-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_group_comes_from_the_password_file() {
        let passwd = "root:x:0:0:System administrator:/root:/bin/sh\n\
            rift:x:1000:100:Rift owner:/home/rift:/bin/fish\n\
            broken:x:1001\n";
        assert_eq!(group_of(passwd, 1000), Some(100));
        assert_eq!(group_of(passwd, 0), Some(0));
        assert_eq!(group_of(passwd, 1001), None);
        assert_eq!(group_of(passwd, 4242), None);
    }

    #[test]
    fn only_plain_paths_inside_home_are_taken() {
        let home = Path::new("/home");
        assert_eq!(
            under_home(home, Path::new("/home/rift/notes.txt")),
            Some(PathBuf::from("rift/notes.txt"))
        );
        for path in [
            "/home",
            "/home/",
            "/etc/shadow",
            "/homework/notes.txt",
            "home/rift/notes.txt",
            "/home/rift/../../etc/shadow",
            "/home/rift/./notes.txt",
            "/home/../etc/shadow",
        ] {
            assert_eq!(under_home(home, Path::new(path)), None, "{path}");
            assert!(
                matches!(inside(home, path), Err(Problem::Invalid(_))),
                "{path}"
            );
        }
        assert_eq!(
            inside(home, "/home/rift/notes.txt"),
            Ok((
                PathBuf::from("rift/notes.txt"),
                PathBuf::from("/home/rift/notes.txt")
            ))
        );
    }

    #[test]
    fn a_deleted_file_comes_back_with_its_mode_and_time() {
        let scratch = Scratch::new("deleted");
        let from = scratch.0.join("snapshot/todo.txt");
        let to = scratch.0.join("home/timeline/todo.txt");
        fs::create_dir_all(from.parent().unwrap()).unwrap();
        fs::write(&from, "Buy milk\n").unwrap();
        fs::set_permissions(&from, fs::Permissions::from_mode(0o640)).unwrap();
        let then = SystemTime::UNIX_EPOCH + Duration::from_secs(1_789_221_603);
        File::options()
            .write(true)
            .open(&from)
            .unwrap()
            .set_modified(then)
            .unwrap();

        assert_eq!(
            copy_back(&from, &to, false, SNAPSHOT),
            Ok(Outcome::Restored)
        );
        assert_eq!(fs::read_to_string(&to).unwrap(), "Buy milk\n");
        let meta = fs::metadata(&to).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o640);
        assert_eq!(meta.modified().unwrap(), then);
        // nothing is left next to it
        assert_eq!(fs::read_dir(to.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn a_changed_file_is_only_overwritten_when_asked() {
        let scratch = Scratch::new("changed");
        let from = scratch.0.join("notes-then.txt");
        let to = scratch.0.join("notes.txt");
        fs::write(&from, "First draft\n").unwrap();
        fs::write(&to, "Second draft\n").unwrap();

        let refused = copy_back(&from, &to, false, SNAPSHOT);
        assert!(
            matches!(&refused, Err(Problem::Changed(why)) if why.ends_with("has changed since this snapshot.")),
            "{refused:?}"
        );
        assert_eq!(fs::read_to_string(&to).unwrap(), "Second draft\n");
        let refused = copy_back(&from, &to, false, Source::Backup);
        assert!(
            matches!(&refused, Err(Problem::Changed(why)) if why.ends_with("has changed since this backup.")),
            "{refused:?}"
        );

        assert_eq!(copy_back(&from, &to, true, SNAPSHOT), Ok(Outcome::Replaced));
        assert_eq!(fs::read_to_string(&to).unwrap(), "First draft\n");
        assert_eq!(
            copy_back(&from, &to, false, SNAPSHOT),
            Ok(Outcome::Unchanged)
        );
    }

    #[test]
    fn a_file_of_the_same_length_with_other_bytes_is_changed() {
        let scratch = Scratch::new("same-length");
        let from = scratch.0.join("a");
        let to = scratch.0.join("b");
        fs::write(&from, "abc").unwrap();
        fs::write(&to, "abd").unwrap();
        assert!(matches!(
            copy_back(&from, &to, false, SNAPSHOT),
            Err(Problem::Changed(_))
        ));
    }

    #[test]
    fn folders_links_and_missing_files_are_refused() {
        let scratch = Scratch::new("refused");
        let folder = scratch.0.join("folder");
        fs::create_dir_all(&folder).unwrap();
        let link = scratch.0.join("link");
        std::os::unix::fs::symlink("/etc/hostname", &link).unwrap();
        let to = scratch.0.join("out");

        assert!(matches!(
            copy_back(&folder, &to, false, SNAPSHOT),
            Err(Problem::Invalid(_))
        ));
        assert!(matches!(
            copy_back(&link, &to, true, SNAPSHOT),
            Err(Problem::Invalid(_))
        ));
        assert_eq!(
            copy_back(&scratch.0.join("gone"), &to, false, Source::Backup),
            Err(Problem::Missing(format!(
                "{} is not in this backup.",
                to.display()
            )))
        );
        assert!(!to.exists());

        let file = scratch.0.join("file");
        fs::write(&file, "x").unwrap();
        assert!(matches!(
            copy_back(&file, &folder, true, SNAPSHOT),
            Err(Problem::Failed(_))
        ));
    }

    #[test]
    fn a_problem_survives_the_exit_status() {
        for problem in [
            Problem::Invalid("a".into()),
            Problem::Missing("b".into()),
            Problem::Changed("c".into()),
            Problem::Failed("d".into()),
        ] {
            let back =
                Problem::from_code(Some(i32::from(problem.code())), problem.sentence().into());
            assert_eq!(back, problem);
        }
        for outcome in [Outcome::Restored, Outcome::Replaced, Outcome::Unchanged] {
            assert_eq!(Outcome::from_name(outcome.name()), Some(outcome));
        }
    }
}
