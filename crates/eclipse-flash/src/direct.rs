//! Writing a drive straight onto a disk or into a file, with no program to partition or format it:
//! how eclipse-flash writes on macOS and Windows. eclipse-flash writes the partition table itself,
//! copies the image's partitions into theirs, and leaves persist for the drive to make at its first
//! boot, since neither system can make LUKS or btrfs.

#![cfg_attr(target_os = "linux", allow(dead_code))]

use std::fs::{File, OpenOptions};
use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};

use libeclipse::disk::run::{self, Drive, copy_to};
use libeclipse::disk::{ALIGN, Bus, Disk, Partition, SECTOR, Table, needed, plan, size, write_gpt};

use crate::Request;
use crate::drive::check_file;
use crate::image::{Image, Reader};

/// What eclipse-flash needs from a system to write a drive onto one of its disks.
pub trait System {
    /// A disk opened to be written.
    type Opened: Drive;

    /// What to do to get the rights to write a disk, when this process does not have them.
    fn rights(&self) -> Option<&'static str>;

    /// The system's disks.
    fn disks(&self) -> &[Disk];

    /// The disk `target` names, `/dev/disk4` or `\\.\PhysicalDrive2`, or nothing when it names a
    /// file.
    fn disk(&self, target: &str) -> Option<Result<Disk, String>>;

    /// Unmounts or clears what is on the disk and opens it to be written.
    ///
    /// # Errors
    ///
    /// When what is on it is still in use, or it cannot be opened.
    fn open(&self, disk: &Disk) -> Result<Self::Opened, String>;

    /// Tells the system the disk has a new partition table, or ejects it.
    fn finish(&self, disk: &Disk);
}

/// Where a drive is written.
enum Target {
    Disk(Disk),
    /// An empty file of this many bytes.
    File(PathBuf, u64),
}

/// A drive opened to be written: what it is shown as, how many bytes it holds, and whether it reads as
/// zeros everywhere, the way a file just emptied does.
pub struct Place<'a, D: Drive> {
    /// The drive.
    pub drive: &'a mut D,
    /// What it is shown as.
    pub path: &'a Path,
    /// Its size.
    pub bytes: u64,
    /// Whether it reads as zeros where nothing was written.
    pub sparse: bool,
}

/// What is in use on a disk and is not let go of for the person: what is mounted from a disk image,
/// which some program attached and may be using. A stick is unmounted before it is written.
fn in_use(disk: &Disk) -> Option<String> {
    (disk.bus == Bus::Image).then(|| disk.mounted()).flatten()
}

/// What `eclipse-flash list` prints: the sticks and USB disks that are plugged in, and why one cannot
/// be written onto now.
pub fn list(system: &impl System) -> Vec<String> {
    let mut lines = Vec::new();
    for disk in system.disks().iter().filter(|disk| disk.listed()) {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(disk.describe());
        if let Err(why) = disk.refuse(needed(0, None), in_use(disk).as_deref()) {
            lines.push(why);
        }
    }
    if lines.is_empty() {
        lines.push("No stick or USB disk is plugged in.".into());
    }
    lines
}

/// `eclipse-flash write`: shows the image and the disk, has its serial or name typed back and writes
/// the drive. `ask` shows a question and returns the answer, or nothing when there is no terminal to
/// ask on.
pub fn write(
    request: &Request,
    system: &impl System,
    ask: &mut impl FnMut(&str) -> Option<String>,
    say: &mut impl FnMut(String),
) -> Result<(), String> {
    if let Some(rights) = system.rights() {
        return Err(rights.into());
    }
    if request.models.is_some() {
        return Err(
            "eclipse-flash cannot copy models onto a drive on this system, since the drive makes persist at its first boot. Nothing was written."
                .into(),
        );
    }
    let image = Image::open(&request.image)?;
    let target = target(request, system)?;
    let version = &image.slot.version;
    say(format!(
        "{} holds Eclipse {version}.",
        request.image.display()
    ));
    confirm(&target, request, ask, say)?;
    let shown = request.target.display();
    let written = match &target {
        Target::Disk(disk) => {
            let mut opened = system.open(disk)?;
            let place = Place {
                drive: &mut opened,
                path: Path::new(&disk.path),
                bytes: disk.size,
                sparse: false,
            };
            let written = write_drive(&image, place, request.exchange, say);
            drop(opened);
            if written.is_ok() {
                system.finish(disk);
            }
            written
        }
        Target::File(path, bytes) => {
            let mut file = empty(path, *bytes)?;
            let place = Place {
                drive: &mut file,
                path,
                bytes: *bytes,
                sparse: true,
            };
            write_drive(&image, place, request.exchange, say)
        }
    };
    written.map_err(|why| {
        format!(
            "{why}\nThe drive was not finished, and {shown} does not boot. Run eclipse-flash again to start over."
        )
    })?;
    say(format!(
        "{shown} is an Eclipse drive now, with version {version}. It makes persist when it first starts, with a passphrase you choose then."
    ));
    Ok(())
}

/// The disk or the file a request names, refused when a drive must not be written onto it.
fn target(request: &Request, system: &impl System) -> Result<Target, String> {
    let need = needed(0, request.exchange);
    let shown = request.target.display();
    if let Some(found) = system.disk(&shown.to_string()) {
        let disk = found?;
        disk.refuse(need, in_use(&disk).as_deref())?;
        return Ok(Target::Disk(disk));
    }
    let kind = request
        .target
        .metadata()
        .map_err(|e| format!("There is no disk or file {shown}: {e}"))?;
    if !kind.is_file() {
        return Err(format!("{shown} is neither a disk nor a file."));
    }
    Ok(Target::File(
        request.target.clone(),
        check_file(&request.target, need)?,
    ))
}

/// Says what is erased, and for a disk has its serial, or its name when it has none, typed back or
/// given with `--serial`.
fn confirm(
    target: &Target,
    request: &Request,
    ask: &mut impl FnMut(&str) -> Option<String>,
    say: &mut impl FnMut(String),
) -> Result<(), String> {
    let disk = match target {
        Target::Disk(disk) => disk,
        Target::File(path, bytes) => {
            if request.serial.is_some() {
                return Err(format!(
                    "{} is a file and has no serial. Nothing was written.",
                    path.display()
                ));
            }
            say(format!(
                "{}, an empty file of {}.",
                path.display(),
                size(*bytes)
            ));
            return Ok(());
        }
    };
    for line in disk.describe() {
        say(line);
    }
    let (what, wanted) = match &disk.serial {
        Some(serial) => ("serial", serial.as_str()),
        None => ("name", disk.name.as_str()),
    };
    let typed = match &request.serial {
        Some(serial) => serial.clone(),
        None => ask(&format!(
            "Everything on it will be erased. To go on, type its {what}, {wanted}: "
        ))
        .ok_or_else(|| {
            format!(
                "Everything on it would be erased. Without a terminal, give its {what} with --serial."
            )
        })?,
    };
    if typed.trim() != wanted {
        return Err(format!(
            "\"{}\" is not the {what} of {}. Nothing was written.",
            typed.trim(),
            disk.path
        ));
    }
    Ok(())
}

/// A file cut to nothing and back to its size, so it reads as zeros, which the copies skip.
fn empty(path: &Path, bytes: u64) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| {
            file.set_len(0)?;
            file.set_len(bytes)?;
            Ok(file)
        })
        .map_err(|e| format!("Could not empty {}: {e}", path.display()))
}

/// Writes a drive: a new partition table without persist, the image's esp and slot A copied into
/// theirs, slot B empty. On a disk the start and the end of the disk, the start of each partition that
/// is not copied and the start of the space after them are zeroed first, so no old table or file
/// system is found there. The table is written last, then read back.
///
/// # Errors
///
/// When reading the image or writing the drive fails, or the table does not read back.
pub fn write_drive(
    image: &Image,
    place: Place<'_, impl Drive>,
    exchange: Option<u64>,
    say: &mut impl FnMut(String),
) -> Result<Table, String> {
    let Place {
        drive,
        path,
        bytes,
        sparse,
    } = place;
    let writing = |e: io::Error| format!("Could not write {}: {e}", path.display());
    let mut seeds = Vec::new();
    for _ in 0..6 {
        seeds.push(run::random()?);
    }
    let mut seeds = seeds.into_iter();
    let table = plan(&image.slot, exchange, &mut || {
        seeds.next().unwrap_or_default()
    });
    let sectors = bytes / SECTOR;
    let gpt = write_gpt(&table, sectors)?;

    say(format!(
        "Writing a new partition table on {}.",
        path.display()
    ));
    if !sparse {
        let mut starts = vec![0, (sectors - ALIGN) * SECTOR];
        starts.extend(
            table.partitions[3..]
                .iter()
                .map(|partition| table.offset(partition)),
        );
        if let Some(last) = table.partitions.last() {
            starts.push((last.start + last.size).div_ceil(ALIGN) * ALIGN * SECTOR);
        }
        for start in starts {
            drive
                .seek(SeekFrom::Start(start))
                .and_then(|_| drive.write_all(&vec![0; ZEROS]))
                .map_err(writing)?;
        }
        drive.sync().map_err(writing)?;
    }

    let mut reader = image.reader()?;
    let mut copy = |reader: &mut Reader,
                    (from, into): (&Partition, &Partition),
                    say: &mut dyn FnMut(String)| {
        reader
            .skip_to(image.table.offset(from))
            .map_err(|e| format!("Could not read {}: {e}", image.path.display()))?;
        copy_to(
            reader,
            drive,
            (&image.path, path),
            (table.offset(into), image.table.bytes(from)),
            sparse,
            &mut |line| say(line),
        )
    };
    say("Copying the boot partition.".into());
    copy(&mut reader, (&image.esp, &table.partitions[0]), &mut |_| {})?;
    say(format!(
        "Copying the system, version {}, {}.",
        image.slot.version,
        size(image.table.bytes(&image.slot.store))
    ));
    copy(
        &mut reader,
        (&image.slot.verity, &table.partitions[1]),
        &mut |_| {},
    )?;
    copy(&mut reader, (&image.slot.store, &table.partitions[2]), say)?;
    reader
        .finish()
        .map_err(|e| format!("Could not read {}: {e}", image.path.display()))?;

    for (offset, bytes) in [(gpt.backup_offset, &gpt.backup), (0, &gpt.primary)] {
        drive
            .seek(SeekFrom::Start(offset))
            .and_then(|_| drive.write_all(bytes))
            .map_err(writing)?;
    }
    drive.sync().map_err(writing)?;
    for (offset, written) in [(0, &gpt.primary), (gpt.backup_offset, &gpt.backup)] {
        let mut read = vec![0; written.len()];
        drive
            .seek(SeekFrom::Start(offset))
            .and_then(|_| drive.read_exact(&mut read))
            .map_err(|e| format!("Could not read {} back: {e}", path.display()))?;
        if &read != written {
            return Err(format!(
                "The partition table on {} does not read back as it was written.",
                path.display()
            ));
        }
    }
    Ok(table)
}

/// What is zeroed at each place, in bytes: a MiB, which holds every table and file system header that
/// could be found there.
const ZEROS: usize = 1 << 20;

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::fs;
    use std::io::{Read, Seek, Write};
    use std::rc::Rc;

    use libeclipse::disk::{GIB, GPT_BYTES, read_gpt};

    use super::*;
    use crate::sample;

    const CHUNK: u64 = 1 << 20;

    /// A disk in memory, a MiB at a time, that remembers where each write went.
    #[derive(Default)]
    struct Memory {
        chunks: HashMap<u64, Vec<u8>>,
        at: u64,
        writes: Vec<(u64, usize)>,
    }

    impl Memory {
        fn read_at(&mut self, offset: u64, count: usize) -> Vec<u8> {
            let mut read = vec![0; count];
            self.at = offset;
            self.read_exact(&mut read).unwrap();
            read
        }
    }

    fn within(at: u64) -> (u64, usize) {
        (at / CHUNK, usize::try_from(at % CHUNK).unwrap())
    }

    impl Read for Memory {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let (chunk, inside) = within(self.at);
            let count = buf.len().min(usize::try_from(CHUNK).unwrap() - inside);
            match self.chunks.get(&chunk) {
                Some(data) => buf[..count].copy_from_slice(&data[inside..inside + count]),
                None => buf[..count].fill(0),
            }
            self.at += u64::try_from(count).unwrap();
            Ok(count)
        }
    }

    impl Write for Memory {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes.push((self.at, buf.len()));
            let mut done = 0;
            while done < buf.len() {
                let (chunk, inside) = within(self.at);
                let count = (buf.len() - done).min(usize::try_from(CHUNK).unwrap() - inside);
                let data = self
                    .chunks
                    .entry(chunk)
                    .or_insert_with(|| vec![0; usize::try_from(CHUNK).unwrap()]);
                data[inside..inside + count].copy_from_slice(&buf[done..done + count]);
                done += count;
                self.at += u64::try_from(count).unwrap();
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Seek for Memory {
        fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
            self.at = match to {
                SeekFrom::Start(at) => at,
                SeekFrom::Current(ahead) => self.at.checked_add_signed(ahead).unwrap(),
                SeekFrom::End(_) => return Err(io::Error::other("a disk has no end to seek to")),
            };
            Ok(self.at)
        }
    }

    impl Drive for Memory {
        fn sync(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The disk in memory, shared with the test that looks at it after.
    struct Shared(Rc<RefCell<Memory>>);

    impl Read for Shared {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.borrow_mut().read(buf)
        }
    }

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Seek for Shared {
        fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
            self.0.borrow_mut().seek(to)
        }
    }

    impl Drive for Shared {
        fn sync(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct Fake {
        disks: Vec<Disk>,
        memory: Rc<RefCell<Memory>>,
        rights: Option<&'static str>,
        opened: Cell<bool>,
        finished: Cell<bool>,
    }

    impl System for Fake {
        type Opened = Shared;

        fn rights(&self) -> Option<&'static str> {
            self.rights
        }

        fn disks(&self) -> &[Disk] {
            &self.disks
        }

        fn disk(&self, target: &str) -> Option<Result<Disk, String>> {
            let name = target.strip_prefix("/dev/")?;
            Some(
                self.disks
                    .iter()
                    .find(|disk| disk.name == name)
                    .cloned()
                    .ok_or_else(|| format!("There is no disk {target}.")),
            )
        }

        fn open(&self, _: &Disk) -> Result<Shared, String> {
            self.opened.set(true);
            Ok(Shared(Rc::clone(&self.memory)))
        }

        fn finish(&self, _: &Disk) {
            self.finished.set(true);
        }
    }

    fn disk(name: &str, bus: Bus) -> Disk {
        Disk {
            name: name.into(),
            path: format!("/dev/{name}"),
            whole: true,
            running: false,
            bus,
            read_only: false,
            sector: Some(512),
            size: 24 * GIB,
            model: Some("Ultra Fit".into()),
            serial: None,
            volumes: vec![libeclipse::disk::Volume {
                path: format!("/dev/{name}s1"),
                name: "STICK".into(),
                fs: None,
                mounted: Some("/Volumes/STICK".into()),
            }],
        }
    }

    fn fake() -> Fake {
        let mut system = disk("disk0", Bus::Inside);
        system.running = true;
        Fake {
            disks: vec![system, disk("disk4", Bus::Usb), disk("disk5", Bus::Image)],
            memory: Rc::new(RefCell::new(Memory::default())),
            rights: None,
            opened: Cell::new(false),
            finished: Cell::new(false),
        }
    }

    fn folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("eclipse-flash-{name}-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn request(image: &Path, target: &str) -> Request {
        Request {
            image: image.to_path_buf(),
            target: target.into(),
            exchange: None,
            models: None,
            serial: None,
        }
    }

    /// Checks that `read` holds the sample image's slot, an empty slot B and both copies of the table,
    /// and returns the table.
    fn check(read: &mut dyn FnMut(u64, usize) -> Vec<u8>, bytes: u64, exchange: bool) -> Table {
        let table = read_gpt(&read(0, GPT_BYTES)).unwrap();
        let mut names = vec![
            "esp",
            "store-verity_0.1.0",
            "store_0.1.0",
            "_empty",
            "_empty",
        ];
        if exchange {
            names.push("exchange");
        }
        let found: Vec<&str> = table
            .partitions
            .iter()
            .map(|partition| partition.name.as_deref().unwrap())
            .collect();
        assert_eq!(found, names);
        let image = sample::bytes();
        let from = sample::table();
        for (index, partition) in from.partitions.iter().enumerate() {
            let written = &table.partitions[index];
            if index > 0 {
                assert_eq!(written.uuid, partition.uuid);
                assert_eq!(written.attrs, partition.attrs);
            }
            assert_eq!(written.kind, partition.kind);
            let start = usize::try_from(from.offset(partition)).unwrap();
            let count = usize::try_from(from.bytes(partition)).unwrap();
            assert!(
                read(table.offset(written), count) == image[start..start + count],
                "partition {index}"
            );
        }
        for partition in &table.partitions[3..] {
            assert!(
                read(table.offset(partition), ZEROS)
                    .iter()
                    .all(|&byte| byte == 0)
            );
        }
        let last = bytes / 512 - 1;
        let backup = read(last * 512, 512);
        assert_eq!(&backup[..8], b"EFI PART");
        let primary = read(512, 512);
        assert_eq!(primary[32..40], last.to_le_bytes());
        let entries = u64::from_le_bytes(backup[72..80].try_into().unwrap());
        assert_eq!(read(entries * 512, 128 * 128), read(1024, 128 * 128));
        table
    }

    #[test]
    fn a_stick_gets_the_image_s_slot_and_an_empty_slot_b() {
        let folder = folder("stick");
        let image = folder.join("sample.raw.zst");
        sample::write(&image).unwrap();
        let system = fake();
        {
            // what the stick held before: a table, a file system, old data where slot b and the
            // space after the exchange partition start
            let mut memory = system.memory.borrow_mut();
            for at in [
                0,
                512 * 40,
                20_973_568 * 512,
                41_945_088 * 512,
                24 * GIB - 512,
            ] {
                memory.at = at;
                memory.write_all(&[0xAA; 512]).unwrap();
            }
            memory.writes.clear();
        }
        let mut exchange = request(&image, "/dev/disk4");
        exchange.exchange = Some(GIB);
        let mut asked = Vec::new();
        let mut said = Vec::new();
        write(
            &exchange,
            &system,
            &mut |question| {
                asked.push(question.to_string());
                Some("disk4\n".into())
            },
            &mut |line| said.push(line),
        )
        .unwrap();
        assert_eq!(
            asked,
            ["Everything on it will be erased. To go on, type its name, disk4: "]
        );
        assert!(system.opened.get() && system.finished.get());
        assert_eq!(said[0], format!("{} holds Eclipse 0.1.0.", image.display()));
        assert_eq!(said[1], "/dev/disk4, Ultra Fit, 24.0 GiB.");
        assert!(said.contains(&"Copying the system, version 0.1.0, 6 MiB.".to_string()));
        assert!(
            said.last()
                .unwrap()
                .starts_with("/dev/disk4 is an Eclipse drive now")
        );

        let mut memory = system.memory.borrow_mut();
        // every write is in whole sectors, as a raw disk on macOS and Windows wants them
        assert!(
            memory
                .writes
                .iter()
                .all(|&(at, count)| at % 512 == 0 && count % 512 == 0),
            "{:?}",
            memory.writes
        );
        let table = check(&mut |at, count| memory.read_at(at, count), 24 * GIB, true);
        // the old bytes after the last partition are gone
        let after = table.partitions[5].start + table.partitions[5].size;
        assert!(
            memory
                .read_at(after * 512, 512)
                .iter()
                .all(|&byte| byte == 0)
        );
        // between the table and the esp too
        assert!(memory.read_at(512 * 40, 512).iter().all(|&byte| byte == 0));
        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn nothing_is_written_onto_a_disk_that_is_refused() {
        let folder = folder("refused");
        let image = folder.join("sample.raw");
        sample::write(&image).unwrap();
        let refused = |system: &Fake, request: &Request, answer: Option<&str>, words: &str| {
            let why = write(
                request,
                system,
                &mut |_| answer.map(str::to_string),
                &mut |_| {},
            )
            .unwrap_err();
            assert!(why.contains(words), "{why}");
            assert!(!system.opened.get(), "{why}");
        };
        let system = fake();
        refused(
            &system,
            &request(&image, "/dev/disk0"),
            Some("disk0"),
            "is the drive this system runs from.",
        );
        refused(
            &system,
            &request(&image, "/dev/disk4"),
            Some("disk5"),
            "\"disk5\" is not the name of /dev/disk4. Nothing was written.",
        );
        refused(
            &system,
            &request(&image, "/dev/disk4"),
            None,
            "Without a terminal, give its name with --serial.",
        );
        refused(
            &system,
            &request(&image, "/dev/disk5"),
            Some("disk5"),
            "/dev/disk5s1 is mounted on /Volumes/STICK. Unmount or close it first.",
        );
        refused(
            &system,
            &request(&image, "/dev/disk9"),
            Some("disk9"),
            "There is no disk /dev/disk9.",
        );
        let mut models = request(&image, "/dev/disk4");
        models.models = Some(folder.clone());
        refused(&system, &models, Some("disk4"), "cannot copy models");
        let short = folder.join("short.img");
        fs::write(&short, [0; 4096]).unwrap();
        refused(
            &system,
            &request(&image, short.to_str().unwrap()),
            None,
            "and Eclipse needs 21.1 GiB.",
        );
        refused(
            &system,
            &request(&image, folder.to_str().unwrap()),
            None,
            "is neither a disk nor a file.",
        );
        let mut locked = fake();
        locked.rights = Some("Writing a drive erases a disk, so it needs root.");
        refused(
            &locked,
            &request(&image, "/dev/disk4"),
            Some("disk4"),
            "needs root",
        );
        assert!(system.memory.borrow().writes.is_empty());
        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn list_shows_sticks_and_why_they_are_refused() {
        let mut system = fake();
        assert_eq!(
            list(&system),
            ["/dev/disk4, Ultra Fit, 24.0 GiB.", "It holds STICK."]
        );
        system.disks[1].size = 16 * GIB;
        system.disks[2].volumes.clear();
        assert_eq!(
            list(&system),
            [
                "/dev/disk4, Ultra Fit, 16.0 GiB.",
                "It holds STICK.",
                "/dev/disk4 holds 16.0 GiB, and Eclipse needs 21.1 GiB.",
                "",
                "/dev/disk5, Ultra Fit, 24.0 GiB.",
                "It has no partitions."
            ]
        );
        system.disks.truncate(1);
        assert_eq!(list(&system), ["No stick or USB disk is plugged in."]);
    }

    // a file of 24G that reads as zeros takes no room on a unix file system, and on NTFS it would
    #[cfg(unix)]
    #[test]
    fn a_file_gets_the_same_drive() {
        let folder = folder("file");
        let image = folder.join("sample.raw");
        sample::write(&image).unwrap();
        let target = folder.join("drive.img");
        File::create(&target).unwrap().set_len(24 * GIB).unwrap();
        let system = fake();
        let mut said = Vec::new();
        write(
            &request(&image, target.to_str().unwrap()),
            &system,
            &mut |_| None,
            &mut |line| said.push(line),
        )
        .unwrap();
        assert!(!system.opened.get());
        assert_eq!(
            said[1],
            format!("{}, an empty file of 24.0 GiB.", target.display())
        );
        let mut file = File::open(&target).unwrap();
        check(
            &mut |at, count| {
                let mut read = vec![0; count];
                file.seek(SeekFrom::Start(at)).unwrap();
                file.read_exact(&mut read).unwrap();
                read
            },
            24 * GIB,
            false,
        );
        // written into once, a file is not empty any more
        let why = write(
            &request(&image, target.to_str().unwrap()),
            &system,
            &mut |_| None,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(why.contains("is not empty"), "{why}");
        fs::remove_dir_all(&folder).unwrap();
    }
}
