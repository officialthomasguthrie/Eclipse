//! eclipse-flash: writes Eclipse onto a stick or a USB disk from a system image. `list` shows the
//! disks it can write onto, `write` erases one and puts the system and an empty slot for the next
//! update onto it. On Linux it makes the encrypted persist too, unless it is asked to leave persist
//! to the drive; a drive written on macOS or Windows makes persist at its first boot. It refuses a
//! disk inside the computer, the disk the running system is on, and a disk that is in use.

mod direct;
mod drive;
mod image;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(test)]
mod sample;
#[cfg(windows)]
mod windows;

use std::path::PathBuf;
use std::process::ExitCode;

#[cfg(target_os = "linux")]
const USAGE: &str = "Usage: eclipse-flash list
       sudo eclipse-flash write [--exchange <size>] [--models <folder> | --first-boot] [--serial <serial>] <image> <disk>";

#[cfg(target_os = "linux")]
const HELP: &str = "Writes Eclipse onto a stick or a USB disk: the system in the image, an empty slot \
for the next update, and persist, encrypted with a passphrase you choose. With --first-boot persist is \
left out, and the drive makes it when it first starts, with a passphrase typed on its own screen. \
Everything on the disk is erased, so eclipse-flash shows the disk first and asks you to type its \
serial. It refuses a disk inside the computer, the disk this system runs from, and a disk that is in \
use. The image is a .raw or .raw.zst file. For a virtual machine the disk can be an empty file \
instead, made with truncate -s 24G <file>. Without a terminal, give the serial with --serial and the \
passphrase on one line of input.

Commands:
  list               Show the sticks and USB disks that are plugged in
  write              Erase a disk and write Eclipse onto it

Options for write:
  --exchange <size>  Add a partition of this size that Windows and macOS can read, like 8G
  --models <folder>  Copy the model files in this folder onto the drive
  --first-boot       Leave persist for the drive to make when it first starts
  --serial <serial>  The disk's serial, given instead of typed";

#[cfg(target_os = "macos")]
const USAGE: &str = "Usage: eclipse-flash list
       sudo eclipse-flash write [--exchange <size>] [--serial <name>] <image> <disk>";

#[cfg(target_os = "macos")]
const HELP: &str = "Writes Eclipse onto a stick or a USB disk: the system in the image and an empty \
slot for the next update. The drive makes persist, encrypted with a passphrase you choose, when it \
first starts. Everything on the disk is erased, so eclipse-flash shows the disk first and asks you to \
type its name. It refuses a disk inside the Mac, the disk macOS runs from, and a disk image that is \
in use, and it unmounts the stick before it writes. The image is a .raw or .raw.zst file. For a \
virtual machine the disk can be an empty file instead, made with mkfile -n 24g <file>. Without a \
terminal, give the name with --serial.

Commands:
  list               Show the sticks and USB disks that are plugged in
  write              Erase a disk and write Eclipse onto it

Options for write:
  --exchange <size>  Add a partition of this size that Windows and macOS can read, like 8G
  --serial <name>    The disk's name, like disk4, given instead of typed";

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const USAGE: &str = "Usage: eclipse-flash list
       eclipse-flash write [--exchange <size>] [--serial <serial>] <image> <disk>";

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const HELP: &str = "Writes Eclipse onto a stick or a USB disk: the system in the image and an empty \
slot for the next update. The drive makes persist, encrypted with a passphrase you choose, when it \
first starts. Everything on the disk is erased, so eclipse-flash shows the disk first and asks you to \
type its serial. It refuses a disk inside the computer, the disk Windows runs from, and a virtual \
disk that is in use, and it removes the partitions on the stick before it writes. Writing needs a \
terminal opened with Run as administrator. A disk is named the way Windows numbers it, like \
PhysicalDrive2, and eclipse-flash list shows the names. The image is a .raw or .raw.zst file. For a \
virtual machine the disk can be an empty file instead, made with fsutil file createnew <file> \
25769803776. Without a terminal, give the serial with --serial.

Commands:
  list               Show the sticks and USB disks that are plugged in
  write              Erase a disk and write Eclipse onto it

Options for write:
  --exchange <size>  Add a partition of this size that Windows and macOS can read, like 8G
  --serial <serial>  The disk's serial, or its name when it has none, given instead of typed";

/// What `write` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The image, `.raw` or `.raw.zst`.
    pub image: PathBuf,
    /// The disk or the empty file the drive goes onto.
    pub target: PathBuf,
    /// The size of the exchange partition, when there is one.
    pub exchange: Option<u64>,
    /// A folder of models to copy into `@models`.
    pub models: Option<PathBuf>,
    /// Whether persist is left for the drive to make at its first boot. On macOS and Windows it
    /// always is.
    pub first_boot: bool,
    /// The disk's serial, given instead of typed.
    pub serial: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    Version,
    List,
    Write(Request),
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Ok(Command::Help) => {
            println!("{USAGE}\n\n{HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("eclipse-flash {}", libeclipse::VERSION);
            ExitCode::SUCCESS
        }
        Ok(command) => match run(&command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(why) => {
                eprintln!("{why}");
                ExitCode::FAILURE
            }
        },
        Err(why) => {
            eprintln!("eclipse-flash: {why}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(target_os = "linux")]
fn run(command: &Command) -> Result<(), String> {
    match command {
        Command::List => linux::list(),
        Command::Write(request) => linux::write(request),
        Command::Help | Command::Version => Ok(()),
    }
}

#[cfg(any(target_os = "macos", windows))]
fn run(command: &Command) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let system = macos::Mac::query()?;
    #[cfg(windows)]
    let system = windows::Windows::query()?;
    match command {
        Command::List => {
            for line in direct::list(&system) {
                println!("{line}");
            }
            Ok(())
        }
        Command::Write(request) => {
            direct::write(request, &system, &mut ask, &mut |line| println!("{line}"))
        }
        Command::Help | Command::Version => Ok(()),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn run(_: &Command) -> Result<(), String> {
    Err("eclipse-flash cannot write a drive on this system.".into())
}

/// A line typed on the terminal after `question`, or nothing when there is no terminal.
#[cfg(any(target_os = "macos", windows))]
fn ask(question: &str) -> Option<String> {
    use std::io::{BufRead, IsTerminal, Write};

    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return None;
    }
    eprint!("{question}");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    stdin.lock().read_line(&mut line).ok()?;
    Some(line.trim_end_matches(['\n', '\r']).to_string())
}

fn parse(args: &[String]) -> Result<Command, String> {
    let mut words = args.iter();
    match words.next().map(String::as_str) {
        None | Some("--help" | "-h" | "help") => return Ok(Command::Help),
        Some("--version") => return Ok(Command::Version),
        Some("list") => {
            return match words.next() {
                None => Ok(Command::List),
                Some(extra) => Err(format!("list takes no arguments, `{extra}` is one")),
            };
        }
        Some("write") => {}
        Some(other) => return Err(format!("unknown command `{other}`")),
    }
    let mut exchange = None;
    let mut models = None;
    let mut first_boot = false;
    let mut serial = None;
    let mut paths = Vec::new();
    while let Some(arg) = words.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--exchange" => exchange = drive::exchange_size(value(&mut words, "--exchange")?)?,
            "--models" => models = Some(PathBuf::from(value(&mut words, "--models")?)),
            "--first-boot" => first_boot = true,
            "--serial" => serial = Some(value(&mut words, "--serial")?.to_string()),
            flag if flag.starts_with('-') => return Err(format!("unknown argument `{flag}`")),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    if first_boot && models.is_some() {
        return Err(
            "--models and --first-boot do not go together, models are copied into persist".into(),
        );
    }
    let [image, target]: [PathBuf; 2] = paths.try_into().map_err(|paths: Vec<PathBuf>| {
        format!(
            "write takes an image and a disk, and {} paths were given",
            paths.len()
        )
    })?;
    Ok(Command::Write(Request {
        image,
        target,
        exchange,
        models,
        first_boot,
        serial,
    }))
}

fn value<'a>(words: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<&'a str, String> {
    words
        .next()
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} needs a value"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(list: &[&str]) -> Result<Command, String> {
        parse(&list.iter().map(ToString::to_string).collect::<Vec<_>>())
    }

    #[test]
    fn write_takes_an_image_a_disk_and_options() {
        assert_eq!(
            parsed(&["write", "eclipse.raw.zst", "/dev/sdb"]),
            Ok(Command::Write(Request {
                image: "eclipse.raw.zst".into(),
                target: "/dev/sdb".into(),
                exchange: None,
                models: None,
                first_boot: false,
                serial: None,
            }))
        );
        assert_eq!(
            parsed(&[
                "write",
                "--exchange",
                "8G",
                "eclipse.raw",
                "--models",
                "models",
                "--serial",
                "4C53",
                "/dev/disk/by-id/usb-Stick"
            ]),
            Ok(Command::Write(Request {
                image: "eclipse.raw".into(),
                target: "/dev/disk/by-id/usb-Stick".into(),
                exchange: Some(8 << 30),
                models: Some("models".into()),
                first_boot: false,
                serial: Some("4C53".into()),
            }))
        );
        assert_eq!(
            parsed(&["write", "--first-boot", "eclipse.raw", "drive.img"]),
            Ok(Command::Write(Request {
                image: "eclipse.raw".into(),
                target: "drive.img".into(),
                exchange: None,
                models: None,
                first_boot: true,
                serial: None,
            }))
        );
        assert_eq!(parsed(&["list"]), Ok(Command::List));
        assert_eq!(parsed(&[]), Ok(Command::Help));
        assert_eq!(parsed(&["write", "--help"]), Ok(Command::Help));
        assert_eq!(parsed(&["--version"]), Ok(Command::Version));
    }

    #[test]
    fn mistakes_are_refused_before_anything_is_looked_at() {
        assert!(parsed(&["write"]).is_err());
        assert!(parsed(&["write", "eclipse.raw"]).is_err());
        assert!(parsed(&["write", "a.raw", "/dev/sdb", "/dev/sdc"]).is_err());
        assert!(parsed(&["write", "--serial"]).is_err());
        assert!(parsed(&["write", "--exchange", "8", "a.raw", "/dev/sdb"]).is_err());
        assert!(parsed(&["write", "--yes", "a.raw", "/dev/sdb"]).is_err());
        assert!(
            parsed(&[
                "write",
                "--first-boot",
                "--models",
                "models",
                "a.raw",
                "/dev/sdb"
            ])
            .is_err()
        );
        assert!(parsed(&["list", "/dev/sdb"]).is_err());
        assert!(parsed(&["erase"]).is_err());
    }
}
