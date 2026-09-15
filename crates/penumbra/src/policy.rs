//! What a sandbox gets: the system's programs read only, the folders asked for at their own paths,
//! and nothing else of the owner's. `check` turns what was asked for into a `Policy`, or a sentence
//! that says why not, and `Policy::bwrap` is the command line that builds the sandbox and starts
//! `penumbra enter` inside it with the same folders as Landlock rules.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

/// The parts of the system every sandbox reads: programs, their libraries and their settings. A
/// system that does not have one leaves it out.
pub const SYSTEM: &[&str] = &[
    "/nix/store",
    "/usr",
    "/bin",
    "/etc",
    "/run/current-system",
    "/run/systemd/resolve",
    "/nix/var/nix/profiles",
];

/// Where homes are, and the one other place a folder can come from.
const HOMES: &str = "/home";
const TMP: &str = "/tmp";

/// The file systems a folder given to a sandbox may be on. Anything else mounted at the folder or
/// inside it, a disk of the computer, a stick or a network share, keeps the folder out.
const MOUNTS: &[&str] = &["/", HOMES, TMP];

/// What was asked for on the command line, before it is checked.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Request {
    /// The folder the command runs in and can change. The current folder when not given.
    pub folder: Option<PathBuf>,
    /// More that the command can read.
    pub read: Vec<PathBuf>,
    /// More that the command can change.
    pub write: Vec<PathBuf>,
    /// The program and its arguments.
    pub command: Vec<String>,
}

/// What a sandbox gets, checked. Every path is absolute with its links followed.
#[derive(Debug, PartialEq, Eq)]
pub struct Policy {
    /// The owner's home. The sandbox has an empty folder in its place that is gone when it ends,
    /// unless all of it is shown read only.
    home: PathBuf,
    /// Shown read only.
    read: Vec<PathBuf>,
    /// Shown and changeable. The first is the folder the command starts in.
    write: Vec<PathBuf>,
}

/// Why a path cannot go into a sandbox.
enum Refusal {
    Outside,
    AllOfHome,
    Mounted(PathBuf),
}

/// Checks a request: every path is resolved and has to be inside the owner's home or inside /tmp,
/// with nothing else mounted at it or inside it. The home as a whole can only be read. `here` is the
/// current folder, `resolve` makes an absolute path with its links followed, and `mounts` are the
/// system's mount points.
///
/// # Errors
///
/// A sentence about the first path that cannot go into a sandbox, or that does not exist.
pub fn check(
    request: &Request,
    home: &Path,
    here: &Path,
    resolve: impl Fn(&Path) -> io::Result<PathBuf>,
    mounts: &[PathBuf],
) -> Result<Policy, String> {
    let find = |path: &Path| {
        resolve(&here.join(path))
            .map_err(|error| format!("Could not find {}: {error}.", path.display()))
    };
    let home = find(home)?;
    if !within(&home, Path::new(HOMES)) {
        return Err(format!(
            "Your home folder is {}, which is not in {HOMES}, so it is not clear what to keep out \
             of the sandbox.",
            home.display()
        ));
    }
    let refusal = |path: &Path, write: bool| {
        if path == home {
            if write {
                return Some(Refusal::AllOfHome);
            }
        } else if !within(path, &home) && !within(path, Path::new(TMP)) {
            return Some(Refusal::Outside);
        }
        mounts
            .iter()
            .filter(|mount| {
                !MOUNTS
                    .iter()
                    .any(|allowed| mount.as_path() == Path::new(allowed))
            })
            .find(|mount| mount.starts_with(path) || path.starts_with(mount))
            .map(|mount| Refusal::Mounted(mount.clone()))
    };
    let given = |path: &Path, write: bool| {
        let path = find(path)?;
        match refusal(&path, write) {
            None => Ok(path),
            Some(refused) => Err(sentence(&path, &refused, true)),
        }
    };

    let folder = if let Some(folder) = &request.folder {
        given(folder, true)?
    } else {
        let here = find(here)?;
        if let Some(refused) = refusal(&here, true) {
            return Err(sentence(&here, &refused, false));
        }
        here
    };
    let mut write = vec![folder];
    for path in &request.write {
        let path = given(path, true)?;
        if !write.contains(&path) {
            write.push(path);
        }
    }
    let mut read = Vec::new();
    for path in &request.read {
        let path = given(path, false)?;
        if !write.contains(&path) && !read.contains(&path) {
            read.push(path);
        }
    }
    Ok(Policy { home, read, write })
}

/// Whether `path` is inside `parent` and not `parent` itself.
fn within(path: &Path, parent: &Path) -> bool {
    path != parent && path.starts_with(parent)
}

/// What a refusal says. `given` is false for the current folder, which nobody typed.
fn sentence(path: &Path, refusal: &Refusal, given: bool) -> String {
    let shown = path.display();
    match (refusal, given) {
        (Refusal::Outside, true) => format!(
            "{shown} cannot go into a sandbox. Only what is inside your home folder or inside \
             {TMP} can."
        ),
        (Refusal::Outside, false) => format!(
            "The sandbox gets the folder you are in, and {shown} cannot go into one. Run it from \
             a folder inside your home folder or inside {TMP}, or give one with --folder."
        ),
        (Refusal::AllOfHome, true) => format!(
            "{shown} is all of your home folder. Give a folder inside it, or show all of it read \
             only with --read {shown}."
        ),
        (Refusal::AllOfHome, false) => "The sandbox gets the folder you are in, and that is all \
             of your home folder. Run it from a folder inside it, or give one with --folder."
            .to_string(),
        (Refusal::Mounted(mount), _) if mount == path => format!(
            "{shown} is where another file system is mounted. A sandbox never gets another disk or \
             file system."
        ),
        (Refusal::Mounted(mount), _) if mount.starts_with(path) => format!(
            "{shown} has {} mounted inside it. A sandbox never gets another disk or file system.",
            mount.display()
        ),
        (Refusal::Mounted(mount), _) => format!(
            "{shown} is on another file system, mounted at {}. A sandbox never gets another disk \
             or file system.",
            mount.display()
        ),
    }
}

impl Policy {
    /// The arguments for bwrap: new namespaces for users, mounts, process ids, IPC and the host
    /// name, with no way to make another user namespace inside; the system read only; a /dev with
    /// no disks in it, a new /proc, and empty /tmp and /home; the folders at their own paths, parents
    /// before what is inside them; then `enter` (this program) with the same folders as Landlock
    /// rules, and the command.
    #[must_use]
    pub fn bwrap(&self, enter: &Path, command: &[String]) -> Vec<OsString> {
        let mut line = Line::default();
        line.words(&[
            "--unshare-all",
            "--share-net",
            "--unshare-user",
            "--disable-userns",
            "--die-with-parent",
        ]);
        for path in SYSTEM {
            line.words(&["--ro-bind-try", path, path]);
        }
        line.words(&["--dev", "/dev", "--proc", "/proc", "--tmpfs", TMP])
            .words(&["--tmpfs", HOMES, "--dir"])
            .word(&self.home);
        let mut shown: Vec<(&PathBuf, bool)> = self
            .read
            .iter()
            .map(|path| (path, false))
            .chain(self.write.iter().map(|path| (path, true)))
            .collect();
        shown.sort_by_key(|(path, _)| path.components().count());
        for (path, write) in shown {
            line.word(if write { "--bind" } else { "--ro-bind" })
                .word(path)
                .word(path);
        }
        if let Some(folder) = self.write.first() {
            line.word("--chdir").word(folder);
        }

        line.word("--").word(enter).word("enter");
        for path in SYSTEM.iter().chain(&["/proc"]) {
            line.words(&["--read", path]);
        }
        line.words(&["--write", "/dev", "--write", TMP]);
        if !self.read.contains(&self.home) {
            line.word("--write").word(&self.home);
        }
        for path in &self.read {
            line.word("--read").word(path);
        }
        for path in &self.write {
            line.word("--write").word(path);
        }
        line.word("--");
        for word in command {
            line.word(word);
        }
        line.0
    }
}

/// A command line being put together.
#[derive(Default)]
struct Line(Vec<OsString>);

impl Line {
    fn word(&mut self, word: impl AsRef<OsStr>) -> &mut Self {
        self.0.push(word.as_ref().to_os_string());
        self
    }

    fn words(&mut self, words: &[&str]) -> &mut Self {
        for word in words {
            self.word(word);
        }
        self
    }
}

/// The mount points in /proc/self/mountinfo, the fifth field of each line, with the octal escapes
/// the kernel writes for spaces, tabs, newlines and backslashes undone.
#[must_use]
pub fn mount_points(mountinfo: &str) -> Vec<PathBuf> {
    mountinfo
        .lines()
        .filter_map(|line| line.split(' ').nth(4))
        .map(unescape)
        .collect()
}

fn unescape(field: &str) -> PathBuf {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        let octal = bytes.get(at + 1..at + 4).filter(|digits| {
            byte == b'\\' && digits.iter().all(|digit| matches!(digit, b'0'..=b'7'))
        });
        if let Some(digits) = octal {
            out.push(digits.iter().fold(0u8, |value, digit| {
                value.wrapping_mul(8).wrapping_add(digit - b'0')
            }));
            at += 4;
        } else {
            out.push(byte);
            at += 1;
        }
    }
    PathBuf::from(OsString::from_vec(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/rift";

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    /// Links on the test system: a folder in home that points at persist, one that points at
    /// another folder in home, and a name that is not there.
    fn resolve(path: &Path) -> io::Result<PathBuf> {
        match path.to_str() {
            Some("/home/rift/project/persist") => Ok(PathBuf::from("/persist/@home")),
            Some("/home/rift/project/notes") => Ok(PathBuf::from("/home/rift/Documents/notes")),
            Some(missing) if missing.ends_with("missing") => {
                Err(io::Error::from(io::ErrorKind::NotFound))
            }
            _ => Ok(path.to_path_buf()),
        }
    }

    fn mounts() -> Vec<PathBuf> {
        paths(&[
            "/",
            "/usr",
            "/nix/store",
            "/home",
            "/persist",
            "/var",
            "/tmp",
            "/run/media/rift/Windows",
        ])
    }

    fn checked(here: &str, request: &Request) -> Result<Policy, String> {
        check(
            request,
            Path::new(HOME),
            Path::new(here),
            resolve,
            &mounts(),
        )
    }

    fn request(folder: Option<&str>, read: &[&str], write: &[&str]) -> Request {
        Request {
            folder: folder.map(PathBuf::from),
            read: paths(read),
            write: paths(write),
            command: vec!["make".to_string()],
        }
    }

    #[test]
    fn the_folder_is_where_it_runs_and_what_it_changes() {
        let policy = checked("/home/rift/project", &request(None, &[], &[])).unwrap();
        assert_eq!(
            policy,
            Policy {
                home: PathBuf::from(HOME),
                read: vec![],
                write: paths(&["/home/rift/project"]),
            }
        );
        let policy = checked(
            "/home/rift/project",
            &request(
                Some("/home/rift/other"),
                &["notes", HOME],
                &["/tmp/build", "."],
            ),
        )
        .unwrap();
        assert_eq!(
            policy,
            Policy {
                home: PathBuf::from(HOME),
                read: paths(&["/home/rift/Documents/notes", HOME]),
                write: paths(&["/home/rift/other", "/tmp/build", "/home/rift/project",]),
            }
        );
    }

    #[test]
    fn a_path_both_read_and_changed_is_changed() {
        let policy = checked(
            "/tmp/work",
            &request(None, &["/tmp/work", "/tmp/data", "/tmp/data"], &[]),
        )
        .unwrap();
        assert_eq!(policy.write, paths(&["/tmp/work"]));
        assert_eq!(policy.read, paths(&["/tmp/data"]));
    }

    #[test]
    fn nothing_outside_home_and_tmp() {
        for path in [
            "/persist",
            "/persist/@home/rift",
            "/dev/nvme0n1",
            "/sys/block",
            "/",
            "/home",
            "/home/other",
            "/tmp",
            "/var/lib/rift",
            "/run/media/rift",
            "/home/rift/project/persist",
        ] {
            let refused = checked("/home/rift/project", &request(None, &[path], &[]));
            assert!(
                refused
                    .as_ref()
                    .is_err_and(|why| why.contains("cannot go into a sandbox")),
                "{path}: {refused:?}"
            );
        }
        let refused = checked("/persist", &request(None, &[], &[])).unwrap_err();
        assert!(refused.starts_with("The sandbox gets the folder you are in, and /persist"));
        assert!(checked("/", &request(None, &[], &[])).is_err());
    }

    #[test]
    fn home_as_a_whole_only_read_only() {
        let refused = checked(HOME, &request(None, &[], &[])).unwrap_err();
        assert!(
            refused.contains("that is all of your home folder"),
            "{refused}"
        );
        let refused = checked("/tmp/work", &request(Some(HOME), &[], &[])).unwrap_err();
        assert!(refused.contains("--read /home/rift"), "{refused}");
        assert!(checked("/tmp/work", &request(None, &[], &[HOME])).is_err());
        assert!(checked("/tmp/work", &request(None, &[HOME], &[])).is_ok());
    }

    #[test]
    fn nothing_with_another_file_system_at_it_or_inside_it() {
        let mut stick = mounts();
        stick.push(PathBuf::from("/home/rift/stick"));
        let with_stick = |here: &str, request: &Request| {
            check(request, Path::new(HOME), Path::new(here), resolve, &stick)
        };
        let refused = with_stick("/tmp/work", &request(None, &[HOME], &[])).unwrap_err();
        assert!(
            refused.contains("/home/rift has /home/rift/stick mounted inside it."),
            "{refused}"
        );
        let refused = with_stick(
            "/tmp/work",
            &request(None, &["/home/rift/stick/photos"], &[]),
        )
        .unwrap_err();
        assert!(
            refused.contains("is on another file system, mounted at /home/rift/stick."),
            "{refused}"
        );
        let refused = with_stick("/home/rift/stick", &request(None, &[], &[])).unwrap_err();
        assert!(
            refused.contains("is where another file system is mounted"),
            "{refused}"
        );
        assert!(
            with_stick(
                "/home/rift/project",
                &request(None, &["/home/rift/Documents"], &[])
            )
            .is_ok()
        );
    }

    #[test]
    fn a_path_that_is_not_there() {
        let refused = checked("/tmp/work", &request(None, &["missing"], &[])).unwrap_err();
        assert!(refused.starts_with("Could not find missing: "), "{refused}");
    }

    #[test]
    fn home_has_to_be_in_home() {
        let refused = check(
            &request(None, &[], &[]),
            Path::new("/root"),
            Path::new("/tmp/work"),
            resolve,
            &mounts(),
        )
        .unwrap_err();
        assert!(refused.contains("not in /home"), "{refused}");
    }

    #[test]
    fn the_bwrap_line() {
        let policy = Policy {
            home: PathBuf::from(HOME),
            read: paths(&[HOME, "/tmp/data"]),
            write: paths(&["/home/rift/project", "/tmp"]),
        };
        let line: Vec<String> = policy
            .bwrap(
                Path::new("/nix/store/x-workspace/bin/penumbra"),
                &["sh".to_string(), "-c".to_string(), "ls /".to_string()],
            )
            .iter()
            .map(|word| word.to_string_lossy().into_owned())
            .collect();
        let text = line.join(" ");
        assert!(text.starts_with(
            "--unshare-all --share-net --unshare-user --disable-userns --die-with-parent \
             --ro-bind-try /nix/store /nix/store --ro-bind-try /usr /usr"
        ));
        assert!(text.contains(
            "--dev /dev --proc /proc --tmpfs /tmp --tmpfs /home --dir /home/rift \
             --bind /tmp /tmp --ro-bind /home/rift /home/rift --ro-bind /tmp/data /tmp/data \
             --bind /home/rift/project /home/rift/project --chdir /home/rift/project \
             -- /nix/store/x-workspace/bin/penumbra enter --read /nix/store"
        ));
        assert!(text.ends_with(
            "--read /nix/var/nix/profiles --read /proc --write /dev --write /tmp \
             --read /home/rift --read /tmp/data --write /home/rift/project --write /tmp \
             -- sh -c ls /"
        ));
        assert!(!text.contains("--write /home/rift "));
        assert_eq!(line.last().map(String::as_str), Some("ls /"));
    }

    #[test]
    fn home_is_changeable_when_it_is_the_empty_one() {
        let policy = Policy {
            home: PathBuf::from(HOME),
            read: vec![],
            write: paths(&["/home/rift/project"]),
        };
        let text: Vec<String> = policy
            .bwrap(Path::new("/bin/penumbra"), &["true".to_string()])
            .iter()
            .map(|word| word.to_string_lossy().into_owned())
            .collect();
        assert!(
            text.join(" ")
                .contains("--write /tmp --write /home/rift --write /home/rift/project -- true")
        );
    }

    #[test]
    fn mountinfo() {
        let mountinfo = "\
22 1 0:21 / / rw,relatime shared:1 - tmpfs tmpfs rw,mode=755
31 22 0:30 /@home /home rw,noatime shared:5 - btrfs /dev/mapper/persist rw
40 31 8:1 / /home/rift/My\\040stick rw - exfat /dev/sda1 rw
41 22 8:2 / /run/media/a\\134b rw - vfat /dev/sda2 rw
";
        assert_eq!(
            mount_points(mountinfo),
            paths(&["/", "/home", "/home/rift/My stick", "/run/media/a\\b"])
        );
    }
}
