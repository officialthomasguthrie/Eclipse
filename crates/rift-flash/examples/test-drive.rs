//! Makes a small image laid out like a real one, and checks a drive rift-flash wrote from it. CI
//! runs it on macOS and Windows, which have no sfdisk to make an image or read a drive.
//!
//! `test-drive image <file>` writes the image, compressed with zstd when the name ends in `.zst`.
//! `test-drive check <drive>` checks what rift-flash wrote from it onto a disk or into a file.

#[path = "../src/sample.rs"]
mod sample;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::ExitCode;

use librift::disk::{GPT_BYTES, read_gpt};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match words.as_slice() {
        ["image", file] => sample::write(Path::new(file))
            .map(|()| None)
            .map_err(|e| format!("Could not write {file}: {e}")),
        ["check", drive] => check(drive),
        _ => Err("Usage: test-drive image <file>\n       test-drive check <drive>".into()),
    };
    match result {
        Ok(line) => {
            if let Some(line) = line {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

/// Reads `count` bytes at `offset`, in whole sectors, the way a raw disk has to be read.
fn read_at(file: &mut File, offset: u64, count: usize) -> Result<Vec<u8>, String> {
    let mut read = vec![0; count];
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(&mut read))
        .map_err(|e| format!("Could not read {count} bytes at {offset}: {e}"))?;
    Ok(read)
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut le = [0; 8];
    le.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(le)
}

/// Checks that `drive` holds the sample image's esp and slot, an empty slot B, and both copies of
/// its partition table.
fn check(drive: &str) -> Result<Option<String>, String> {
    let mut file = File::open(drive).map_err(|e| format!("Could not open {drive}: {e}"))?;
    let start = read_at(&mut file, 0, GPT_BYTES)?;
    let table = read_gpt(&start).map_err(|why| format!("{drive}: {why}"))?;
    let names: Vec<&str> = table
        .partitions
        .iter()
        .map(|partition| partition.name.as_deref().unwrap_or(""))
        .collect();
    if !names.starts_with(&[
        "esp",
        "store-verity_0.1.0",
        "store_0.1.0",
        "_empty",
        "_empty",
    ]) || names.len() > 6
        || names.get(5).is_some_and(|name| *name != "exchange")
    {
        return Err(format!("{drive} has the partitions {}.", names.join(", ")));
    }

    let image = sample::bytes();
    let from = sample::table();
    for (index, partition) in from.partitions.iter().enumerate() {
        let written = &table.partitions[index];
        if written.kind != partition.kind || (index > 0 && written.uuid != partition.uuid) {
            return Err(format!(
                "Partition {} on {drive} is not the image's.",
                index + 1
            ));
        }
        let begin = usize::try_from(from.offset(partition)).unwrap_or(usize::MAX);
        let count = usize::try_from(from.bytes(partition)).unwrap_or(usize::MAX);
        if read_at(&mut file, table.offset(written), count)? != image[begin..begin + count] {
            return Err(format!(
                "Partition {} on {drive} does not hold what the image's does.",
                index + 1
            ));
        }
    }
    for partition in &table.partitions[3..] {
        if read_at(&mut file, table.offset(partition), 1 << 20)?
            .iter()
            .any(|&byte| byte != 0)
        {
            return Err(format!("{} on {drive} is not empty.", partition.node));
        }
    }

    let backup = read_at(&mut file, u64_at(&start, 512 + 32) * 512, 512)?;
    let entries = read_at(&mut file, u64_at(&backup, 72) * 512, 128 * 128)?;
    if &backup[..8] != b"EFI PART" || entries != start[1024..1024 + 128 * 128] {
        return Err(format!(
            "The backup partition table on {drive} is not the first one."
        ));
    }
    Ok(Some(format!(
        "{drive} holds the image's esp and slot, an empty slot B{}, and both copies of its partition table.",
        if names.len() == 6 {
            ", an exchange partition"
        } else {
            ""
        }
    )))
}
