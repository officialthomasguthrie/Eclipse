//! A small image laid out like the one the flake builds, for tests, and for CI on systems that have no
//! sfdisk to make one: an esp, a verity partition and a store, each with a pattern of its own.

use std::fs;
use std::io;
use std::path::Path;

use librift::disk::{ESP_TYPE, Partition, Table, USR_TYPE, USR_VERITY_TYPE, write_gpt};
use ruzstd::encoding::{CompressionLevel, compress_to_vec};

/// The sectors of the image: 14 MiB.
const SECTORS: u64 = 28_672;

/// The partition table of the sample image, which holds version 0.1.0.
pub fn table() -> Table {
    let partition =
        |number: u8, (start, size): (u64, u64), kind: &str, uuid: &str, name: &str| Partition {
            node: number.to_string(),
            start,
            size,
            kind: kind.to_string(),
            uuid: Some(uuid.to_string()),
            name: Some(name.to_string()),
            attrs: (number > 1).then(|| "GUID:60".to_string()),
        };
    Table {
        id: Some("B581FEF7-24ED-4F31-990B-099EC86BBA03".into()),
        sectorsize: 512,
        partitions: vec![
            partition(
                1,
                (2048, 8192),
                ESP_TYPE,
                "DAC4058A-2FC0-4808-A7EF-B8B4501C00CE",
                "esp",
            ),
            partition(
                2,
                (10_240, 4096),
                USR_VERITY_TYPE,
                "D47A8035-B6EE-3CD9-799B-BB886EFAEAFB",
                "store-verity_0.1.0",
            ),
            partition(
                3,
                (14_336, 12_288),
                USR_TYPE,
                "229D80B9-7A5A-031B-79F2-324122B3A4F0",
                "store_0.1.0",
            ),
        ],
    }
}

fn index(bytes: u64) -> usize {
    usize::try_from(bytes).unwrap_or(usize::MAX)
}

/// The bytes of the sample image: its table, a pattern of its own in each partition, and a MiB of
/// zeros in the store, the way a real store has them.
pub fn bytes() -> Vec<u8> {
    let table = table();
    let gpt = write_gpt(&table, SECTORS).unwrap_or_else(|why| panic!("{why}"));
    let mut image = vec![0; index(SECTORS * 512)];
    image[..gpt.primary.len()].copy_from_slice(&gpt.primary);
    let backup = index(gpt.backup_offset);
    image[backup..backup + gpt.backup.len()].copy_from_slice(&gpt.backup);
    for (number, partition) in table.partitions.iter().enumerate() {
        let start = index(table.offset(partition));
        let end = start + index(table.bytes(partition));
        for (at, byte) in image[start..end].iter_mut().enumerate() {
            *byte = u8::try_from((at / 512 + number * 31 + at) % 251 + 1).unwrap_or(1);
        }
    }
    let zeros = index(table.offset(&table.partitions[2])) + (1 << 20);
    image[zeros..zeros + (1 << 20)].fill(0);
    image
}

/// Writes the sample image to `path`, compressed with zstd when its name ends in `.zst`.
pub fn write(path: &Path) -> io::Result<()> {
    let bytes = bytes();
    if path.extension().is_some_and(|extension| extension == "zst") {
        fs::write(path, compress_to_vec(&bytes[..], CompressionLevel::Fastest))
    } else {
        fs::write(path, bytes)
    }
}
