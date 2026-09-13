//! Writing a drive on Linux: the disk or the file it goes onto and the rules for each, then the
//! steps, through the same programs Vault's clone runs.

use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, BufRead, IsTerminal, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use libeclipse::disk::run::{
    self, Drive, Loop, Persist, copy, copy_file, feed, on_path, settle, tool,
};
use libeclipse::disk::{
    Block, LSBLK, Partition, Table, confirmation, describe, disks_in, needed, partition_node,
    passphrase_problem, read_blocks, read_lsblk, read_table, refuse, script, size,
};

use crate::direct::{self, Place};
use crate::drive::{check_file, check_table, models_in};
use crate::image::{Image, Reader};
use crate::{Listed, Request};

/// The programs every write runs, looked for before anything is erased.
const TOOLS: [&str; 8] = [
    "lsblk",
    "findmnt",
    "sfdisk",
    "mount",
    "umount",
    "cryptsetup",
    "mkfs.btrfs",
    "btrfs",
];
/// Where persist is mounted while it is filled.
const RUN: &str = "/run/eclipse-flash";
/// What a running system is mounted from. The disks under these are never written.
const SYSTEM: [&str; 6] = ["/", "/usr", "/boot", "/efi", "/nix/store", "/persist"];

/// What a drive is written onto.
#[derive(Debug)]
enum Target {
    /// A whole disk, as lsblk sees it.
    Disk(Box<Block>),
    /// An empty file of this many bytes, for a virtual machine.
    File(PathBuf, u64),
}

impl Target {
    fn path(&self) -> &Path {
        match self {
            Target::Disk(disk) => Path::new(&disk.path),
            Target::File(path, _) => path,
        }
    }

    fn shown(&self) -> Vec<String> {
        match self {
            Target::Disk(disk) => describe(disk),
            Target::File(path, bytes) => vec![format!(
                "{}, an empty file of {}.",
                path.display(),
                size(*bytes)
            )],
        }
    }

    /// Where the bytes of a partition are written: its device on a disk, its offset in a file.
    fn place(&self, table: &Table, partition: &Partition) -> (PathBuf, u64) {
        match self {
            Target::Disk(_) => (PathBuf::from(&partition.node), 0),
            Target::File(path, _) => (path.clone(), table.offset(partition)),
        }
    }

    /// A device to make a file system on: the partition on a disk, a loop device over its bytes in
    /// a file.
    fn device(&self, table: &Table, partition: &Partition) -> Result<Device, String> {
        match self {
            Target::Disk(_) => Ok(Device::Node(PathBuf::from(&partition.node))),
            Target::File(path, _) => {
                Loop::attach(path, table.offset(partition), table.bytes(partition))
                    .map(Device::Loop)
            }
        }
    }
}

enum Device {
    Node(PathBuf),
    Loop(Loop),
}

impl Device {
    fn path(&self) -> &Path {
        match self {
            Device::Node(node) => node,
            Device::Loop(device) => device.path(),
        }
    }

    fn release(self) -> Result<(), String> {
        match self {
            Device::Node(_) => Ok(()),
            Device::Loop(device) => device.detach(),
        }
    }
}

/// Everything a write needs, checked before anything is erased.
struct Plan {
    image: Image,
    target: Target,
    exchange: Option<u64>,
    models: Vec<PathBuf>,
}

/// The sticks and USB disks that are plugged in, and why one cannot be written onto now.
pub fn listed() -> Result<Vec<Listed>, String> {
    let blocks = read_blocks(&tool(
        Command::new("lsblk").args(["--json", "--bytes", "--output", LSBLK]),
    )?)?;
    let running = running_disks();
    Ok(blocks
        .iter()
        .filter_map(|block| {
            let disk = block.disk(&running);
            disk.listed().then(|| Listed {
                refused: refuse(block, &running, needed(0, None)).err(),
                disk,
            })
        })
        .collect())
}

/// A whole disk opened for eclipse-flash alone. Its blocks are dropped from the page cache before it
/// is read back, so the read back reads the disk.
struct Exclusive(File);

impl Read for Exclusive {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Exclusive {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Seek for Exclusive {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.0.seek(to)
    }
}

impl Drive for Exclusive {
    fn sync(&mut self) -> io::Result<()> {
        self.0.sync_data()
    }

    fn uncache(&mut self) -> io::Result<()> {
        uncache(&self.0)
    }
}

/// Drops what the page cache holds of a file or a disk. Only blocks that are on the disk already are
/// dropped, so it comes after a sync.
fn uncache(file: &File) -> io::Result<()> {
    rustix::fs::fadvise(file, 0, None, rustix::fs::Advice::DontNeed).map_err(io::Error::from)
}

/// `sudo eclipse-flash write`: shows the image and the disk, asks for the serial and the passphrase,
/// and writes the drive.
pub fn write(request: &Request) -> Result<(), String> {
    if !rustix::process::geteuid().is_root() {
        return Err(
            "Writing a drive erases a disk, so it needs root. Run sudo eclipse-flash write <image> <disk>."
                .into(),
        );
    }
    let plan = inspect(request)?;
    let path = plan.target.path().display().to_string();
    let version = &plan.image.slot.version;
    println!("{} holds Eclipse {version}.", plan.image.path.display());
    for line in plan.target.shown() {
        println!("{line}");
    }
    let terminal = io::stdin().is_terminal();
    match (&plan.target, &request.serial) {
        (Target::Disk(disk), serial) => {
            let wanted = confirmation(disk);
            let typed = match serial {
                Some(serial) => serial.clone(),
                None if terminal => read_line(
                    &format!(
                        "Everything on it will be erased. To go on, type its serial, {wanted}: "
                    ),
                    false,
                )?,
                None => {
                    return Err(
                        "Everything on it would be erased. Without a terminal, give its serial with --serial."
                            .into(),
                    );
                }
            };
            if typed.trim() != wanted {
                return Err(format!(
                    "\"{}\" is not the serial of {path}. Nothing was written.",
                    typed.trim()
                ));
            }
        }
        (Target::File(..), Some(_)) => {
            return Err(format!(
                "{path} is a file and has no serial. Nothing was written."
            ));
        }
        (Target::File(..), None) => {}
    }
    let unfinished = |why: String| {
        format!(
            "{why}\nThe drive was not finished, and {path} does not boot. Run eclipse-flash again to start over."
        )
    };
    if request.first_boot {
        write_without_persist(&plan, &mut |line| println!("{line}")).map_err(unfinished)?;
        println!(
            "{path} is an Eclipse drive now, with version {version}. It makes persist when it first starts, with a passphrase you choose then."
        );
        return Ok(());
    }
    let passphrase = read_line("Passphrase for persist: ", true)?;
    if let Some(problem) = passphrase_problem(&passphrase) {
        return Err(format!("{problem} Nothing was written."));
    }
    if terminal && read_line("Type it again: ", true)? != passphrase {
        return Err("The two passphrases are not the same. Nothing was written.".into());
    }
    write_drive(&plan, &passphrase, &mut |line| println!("{line}")).map_err(unfinished)?;
    println!(
        "{path} is an Eclipse drive now, with version {version}. Persist opens with the passphrase you chose."
    );
    Ok(())
}

/// Writes the drive without persist, the way macOS and Windows write one, and leaves persist for the
/// drive to make at its first boot.
fn write_without_persist(plan: &Plan, say: &mut impl FnMut(String)) -> Result<(), String> {
    match &plan.target {
        Target::File(path, bytes) => {
            let mut file = direct::empty(path, *bytes)?;
            let place = Place {
                drive: &mut file,
                path,
                bytes: *bytes,
                sparse: true,
            };
            direct::write_drive(&plan.image, place, plan.exchange, say).map(|_| ())
        }
        Target::Disk(disk) => {
            let path = Path::new(&disk.path);
            // exclusive: the open fails while anything on the disk is mounted or opened
            let exclusive = i32::try_from(rustix::fs::OFlags::EXCL.bits())
                .map_err(|e| format!("Could not open {}: {e}", path.display()))?;
            let mut opened = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(exclusive)
                .open(path)
                .map(Exclusive)
                .map_err(|e| format!("Could not open {} to write it: {e}", path.display()))?;
            let place = Place {
                drive: &mut opened,
                path,
                bytes: disk.size,
                sparse: false,
            };
            let table = direct::write_drive(&plan.image, place, plan.exchange, say)?;
            drop(opened);
            tool(Command::new("blockdev").arg("--rereadpt").arg(path))?;
            settle(
                &(1..=table.partitions.len())
                    .map(|number| PathBuf::from(partition_node(&disk.path, number)))
                    .collect::<Vec<_>>(),
            )
        }
    }
}

/// Looks at the image and the target, and refuses them or says what will be written.
fn inspect(request: &Request) -> Result<Plan, String> {
    if let Some(missing) = TOOLS.iter().find(|name| !on_path(name)) {
        return Err(format!(
            "{missing} is missing, and writing a drive needs it."
        ));
    }
    if request.exchange.is_some() && !request.first_boot && !on_path("mkfs.exfat") {
        return Err("mkfs.exfat is missing, and the exchange partition needs it.".into());
    }
    let image = Image::open(&request.image)?;
    let (models, model_bytes) = match &request.models {
        Some(folder) => models_in(folder)?,
        None => (Vec::new(), 0),
    };
    let need = needed(model_bytes, request.exchange);
    let path = fs::canonicalize(&request.target)
        .map_err(|e| format!("There is no disk or file {}: {e}", request.target.display()))?;
    let kind = fs::metadata(&path)
        .map_err(|e| format!("Could not look at {}: {e}", path.display()))?
        .file_type();
    let target = if kind.is_block_device() {
        let disk = read_lsblk(&tool(
            Command::new("lsblk")
                .args(["--json", "--bytes", "--output", LSBLK])
                .arg(&path),
        )?)?;
        refuse(&disk, &running_disks(), need)?;
        Target::Disk(Box::new(disk))
    } else if kind.is_file() {
        let bytes = check_file(&path, need)?;
        Target::File(path, bytes)
    } else {
        return Err(format!("{} is neither a disk nor a file.", path.display()));
    };
    Ok(Plan {
        image,
        target,
        exchange: request.exchange,
        models,
    })
}

/// The names of the disks the running system is on, found from the sources and the device numbers
/// of what it is mounted from.
fn running_disks() -> Vec<String> {
    let mut disks = Vec::new();
    for point in SYSTEM {
        // findmnt fails when nothing is mounted there
        let Ok(printed) = tool(Command::new("findmnt").args([
            "--noheadings",
            "--nofsroot",
            "--output",
            "SOURCE,MAJ:MIN",
            "--mountpoint",
            point,
        ])) else {
            continue;
        };
        for line in printed.lines() {
            let mut words = line.split_whitespace();
            let mut nodes = Vec::new();
            if let Some(source) = words.next().filter(|source| source.starts_with("/dev/")) {
                nodes.push(PathBuf::from(source));
            }
            // a btrfs or a tmpfs has a device number that is no block device, and then this is empty
            if let Some(node) = words
                .next()
                .and_then(|number| fs::canonicalize(Path::new("/sys/dev/block").join(number)).ok())
                .and_then(|device| device.file_name().map(|name| Path::new("/dev").join(name)))
            {
                nodes.push(node);
            }
            for node in nodes.into_iter().filter(|node| node.exists()) {
                if let Ok(list) = tool(
                    Command::new("lsblk")
                        .args([
                            "--inverse",
                            "--list",
                            "--noheadings",
                            "--output",
                            "NAME,TYPE",
                        ])
                        .arg(&node),
                ) {
                    disks.extend(disks_in(&list));
                }
            }
        }
    }
    disks.sort();
    disks.dedup();
    disks
}

/// Erases the target and writes the drive onto it, saying each step.
fn write_drive(plan: &Plan, passphrase: &str, say: &mut impl FnMut(String)) -> Result<(), String> {
    let table = write_table(plan, say)?;
    copy_system(plan, &table, say)?;
    if plan.exchange.is_some() {
        say("Formatting the exchange partition.".into());
        let device = plan.target.device(&table, &table.partitions[5])?;
        tool(
            Command::new("mkfs.exfat")
                .args(["-L", "EXCHANGE"])
                .arg(device.path()),
        )?;
        device.release()?;
    }
    let last = table
        .partitions
        .last()
        .ok_or("sfdisk wrote no partitions.")?;
    say("Encrypting persist.".into());
    let device = plan.target.device(&table, last)?;
    let pid = std::process::id();
    let persist = Persist::make(
        device.path(),
        passphrase,
        &format!("eclipse-flash-{pid}"),
        Path::new(RUN).join(format!("persist-{pid}")),
    )?;
    fill(&persist, &plan.models, say)?;
    persist.close()?;
    device.release()?;
    read_back(plan, &table, say)
}

/// Reads the esp and the slot back from the drive and compares them with the image. It comes after
/// everything else is written, so a stick that wraps around is found whatever landed on the slot.
fn read_back(plan: &Plan, table: &Table, say: &mut impl FnMut(String)) -> Result<(), String> {
    say(format!(
        "Reading {} back to check it.",
        plan.target.path().display()
    ));
    let image = &plan.image;
    let mut check = image.check()?;
    for (number, from) in [&image.esp, &image.slot.verity, &image.slot.store]
        .into_iter()
        .enumerate()
    {
        let (path, offset) = plan.target.place(table, &table.partitions[number]);
        let mut opened = File::open(&path)
            .and_then(|file| uncache(&file).map(|()| file))
            .map_err(|e| format!("Could not read {} back: {e}", path.display()))?;
        if number == 2 {
            check.partition(from, &mut opened, (&path, offset), say)?;
        } else {
            check.partition(from, &mut opened, (&path, offset), &mut |_| {})?;
        }
    }
    Ok(())
}

/// Empties a file target, writes the new partition table and reads it back.
fn write_table(plan: &Plan, say: &mut impl FnMut(String)) -> Result<Table, String> {
    let target = plan.target.path();
    if let Target::File(path, bytes) = &plan.target {
        // cut to nothing and back to its size, a file reads as zeros, which a sparse copy skips
        OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|file| {
                file.set_len(0)?;
                file.set_len(*bytes)
            })
            .map_err(|e| format!("Could not empty {}: {e}", path.display()))?;
    }
    say(format!(
        "Writing a new partition table on {}.",
        target.display()
    ));
    feed(
        Command::new("sfdisk").args(run::sfdisk_args(target)),
        &script(&plan.image.slot, plan.exchange),
    )?;
    if let Target::Disk(disk) = &plan.target {
        let count = if plan.exchange.is_some() { 7 } else { 6 };
        settle(
            &(1..=count)
                .map(|number| PathBuf::from(partition_node(&disk.path, number)))
                .collect::<Vec<_>>(),
        )?;
    }
    let table = read_table(&tool(Command::new("sfdisk").arg("--json").arg(target))?)?;
    check_table(&table, &plan.image.slot, plan.exchange.is_some())?;
    Ok(table)
}

/// Copies the image's esp, verity partition and store into the drive's, reading the image once.
fn copy_system(plan: &Plan, table: &Table, say: &mut impl FnMut(String)) -> Result<(), String> {
    let image = &plan.image;
    let mut reader = image.reader()?;
    say("Copying the boot partition.".into());
    copy_partition(
        plan,
        table,
        &mut reader,
        (&image.esp, &table.partitions[0]),
        &mut |_: String| {},
    )?;
    say(format!(
        "Copying the system, version {}, {}.",
        image.slot.version,
        size(image.table.bytes(&image.slot.store))
    ));
    copy_partition(
        plan,
        table,
        &mut reader,
        (&image.slot.verity, &table.partitions[1]),
        &mut |_: String| {},
    )?;
    copy_partition(
        plan,
        table,
        &mut reader,
        (&image.slot.store, &table.partitions[2]),
        say,
    )?;
    reader
        .finish()
        .map_err(|e| format!("Could not read {}: {e}", image.path.display()))
}

/// Copies one partition of the image into one of the drive's. On a file zeros are skipped, the file
/// was emptied first.
fn copy_partition(
    plan: &Plan,
    table: &Table,
    reader: &mut Reader,
    (from, to): (&Partition, &Partition),
    say: &mut impl FnMut(String),
) -> Result<(), String> {
    let image = &plan.image;
    let bytes = image.table.bytes(from);
    if bytes > table.bytes(to) {
        return Err(format!(
            "Partition {} of the image has {}, and partition {} on the drive only {}.",
            from.node,
            size(bytes),
            to.node,
            size(table.bytes(to))
        ));
    }
    reader
        .skip_to(image.table.offset(from))
        .map_err(|e| format!("Could not read {}: {e}", image.path.display()))?;
    let (path, offset) = plan.target.place(table, to);
    let sparse = matches!(plan.target, Target::File(..));
    copy(reader, &image.path, &path, offset, bytes, sparse, say)
}

/// Makes the subvolumes of persist, the machine id and the owner's home, and copies the models in.
fn fill(persist: &Persist, models: &[PathBuf], say: &mut impl FnMut(String)) -> Result<(), String> {
    persist.fill()?;
    for model in models {
        let Some(name) = model.file_name() else {
            continue;
        };
        say(format!("Copying {} into models.", name.to_string_lossy()));
        let copied = persist.top().join("@models").join(name);
        copy_file(model, &copied)?;
        fs::set_permissions(&copied, Permissions::from_mode(0o644))
            .map_err(|e: io::Error| format!("Could not make {}: {e}", copied.display()))?;
    }
    Ok(())
}

/// A line typed on a terminal after `prompt`, without echo when `hidden`, or one line of stdin.
fn read_line(prompt: &str, hidden: bool) -> Result<String, String> {
    use rustix::termios::{self, LocalModes, OptionalActions};

    let stdin = io::stdin();
    let before = if stdin.is_terminal() {
        eprint!("{prompt}");
        let _ = io::stderr().flush();
        let before = termios::tcgetattr(&stdin).ok().filter(|_| hidden);
        if let Some(mut quiet) = before.clone() {
            quiet.local_modes.remove(LocalModes::ECHO);
            let _ = termios::tcsetattr(&stdin, OptionalActions::Now, &quiet);
        }
        before
    } else {
        None
    };
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    if let Some(before) = before {
        let _ = termios::tcsetattr(&stdin, OptionalActions::Now, &before);
        eprintln!();
    }
    read.map_err(|e| format!("Could not read what was typed: {e}"))?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}
