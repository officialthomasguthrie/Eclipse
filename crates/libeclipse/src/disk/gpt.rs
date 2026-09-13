//! Reading a GUID partition table out of the first bytes of a disk image, without a program to ask.
//! eclipse-flash reads an image's table this way, out of a compressed image too, and on systems that
//! have no sfdisk.

use std::fmt::Write as _;

use super::table::{Partition, Table};

/// How many bytes at the start of an image hold its partition table: the protective MBR, the header
/// and the entries, all before the first partition at 1 MiB.
pub const GPT_BYTES: usize = 1 << 20;

const SIGNATURE: &[u8] = b"EFI PART";
/// The attribute bits that have names, the way sfdisk writes them. Bits 48 to 63 belong to the
/// partition type and are written as `GUID:<bit>,<bit>`.
const NAMED_BITS: [(u32, &str); 3] = [
    (0, "RequiredPartition"),
    (1, "NoBlockIOProtocol"),
    (2, "LegacyBIOSBootable"),
];

/// The partition table at the start of an image, with the partitions numbered from 1 in the order of
/// its entries, and sizes and uuids the way sfdisk prints them.
///
/// # Errors
///
/// When the bytes hold no GUID partition table, or one whose checksums do not match.
pub fn read_gpt(bytes: &[u8]) -> Result<Table, String> {
    [512, 4096]
        .into_iter()
        .find(|&sector| bytes.get(sector..sector + SIGNATURE.len()) == Some(SIGNATURE))
        .ok_or_else(|| "It has no GUID partition table.".to_string())
        .and_then(|sector| read_at(bytes, sector))
}

fn read_at(bytes: &[u8], sector: usize) -> Result<Table, String> {
    let header = bytes
        .get(sector..2 * sector)
        .ok_or("Its partition table header is cut off.")?;
    let header_size = usize::try_from(u32_at(header, 12)).unwrap_or(usize::MAX);
    if !(92..=sector).contains(&header_size) {
        return Err(format!(
            "Its partition table header says it has {header_size} bytes."
        ));
    }
    let mut blank = header[..header_size].to_vec();
    blank[16..20].fill(0);
    if crc32(&blank) != u32_at(header, 16) {
        return Err("Its partition table header is damaged.".into());
    }

    let count = usize::try_from(u32_at(header, 80)).unwrap_or(usize::MAX);
    let entry_size = usize::try_from(u32_at(header, 84)).unwrap_or(usize::MAX);
    if !(128..=4096).contains(&entry_size) || count > 1024 {
        return Err(format!(
            "Its partition table has {count} entries of {entry_size} bytes."
        ));
    }
    let entries = usize::try_from(u64_at(header, 72))
        .ok()
        .and_then(|lba| lba.checked_mul(sector))
        .and_then(|start| bytes.get(start..start.checked_add(count * entry_size)?))
        .ok_or("Its partition entries do not lie before the first partition.")?;
    if crc32(entries) != u32_at(header, 88) {
        return Err("Its partition entries are damaged.".into());
    }

    let mut partitions = Vec::new();
    for (index, entry) in entries.chunks_exact(entry_size).enumerate() {
        if entry[..16].iter().all(|&byte| byte == 0) {
            continue;
        }
        let (first, last) = (u64_at(entry, 32), u64_at(entry, 40));
        if last < first {
            return Err(format!("Partition {} ends before it starts.", index + 1));
        }
        let units: Vec<u16> = entry[56..128]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|&unit| unit != 0)
            .collect();
        let name = String::from_utf16_lossy(&units);
        partitions.push(Partition {
            node: (index + 1).to_string(),
            start: first,
            size: last - first + 1,
            kind: guid(&entry[..16]),
            uuid: Some(guid(&entry[16..32])),
            name: (!name.is_empty()).then_some(name),
            attrs: attributes(u64_at(entry, 48)),
        });
    }
    Ok(Table {
        sectorsize: u64::try_from(sector).unwrap_or(512),
        partitions,
    })
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from(u32_at(bytes, at)) | (u64::from(u32_at(bytes, at + 4)) << 32)
}

/// A GUID as text. The first three fields are stored little endian, the last two as they read.
fn guid(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(36);
    for (place, index) in [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15]
        .into_iter()
        .enumerate()
    {
        if matches!(place, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        let _ = write!(text, "{:02X}", bytes[index]);
    }
    text
}

fn attributes(bits: u64) -> Option<String> {
    let set = |bit: u32| (bits >> bit) & 1 == 1;
    let mut words: Vec<String> = NAMED_BITS
        .iter()
        .filter(|(bit, _)| set(*bit))
        .map(|(_, name)| (*name).to_string())
        .collect();
    let numbered: Vec<String> = (48..64)
        .filter(|&bit| set(bit))
        .map(|bit| bit.to_string())
        .collect();
    if !numbered.is_empty() {
        words.push(format!("GUID:{}", numbered.join(",")));
    }
    (!words.is_empty()).then(|| words.join(" "))
}

/// The CRC-32 GPT uses, the one zlib and Ethernet use.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::super::read_table;
    use super::*;

    /// What sfdisk 2.42 printed for `eclipse_0.1.0.raw`, the image the flake builds.
    const IMAGE: &str = r#"{
       "partitiontable": {
          "label": "gpt", "id": "B581FEF7-24ED-4F31-990B-099EC86BBA03", "device": "result/eclipse_0.1.0.raw",
          "unit": "sectors", "firstlba": 2048, "lastlba": 16201215, "sectorsize": 512,
          "partitions": [
             {"node": "1", "start": 2048, "size": 2097152, "type": "C12A7328-F81F-11D2-BA4B-00A0C93EC93B",
              "uuid": "DAC4058A-2FC0-4808-A7EF-B8B4501C00CE", "name": "esp"},
             {"node": "2", "start": 2099200, "size": 2097152, "type": "77FF5F63-E7B6-4633-ACF4-1565B864C0E6",
              "uuid": "D47A8035-B6EE-3CD9-799B-BB886EFAEAFB", "name": "store-verity_0.1.0", "attrs": "GUID:60"},
             {"node": "3", "start": 4196352, "size": 12004864, "type": "8484680C-9521-48C6-9C11-B0720656F69E",
              "uuid": "229D80B9-7A5A-031B-79F2-324122B3A4F0", "name": "store_0.1.0", "attrs": "GUID:60"}
          ]
       }
    }"#;

    /// A GUID in the byte order it has on disk.
    fn guid_bytes(text: &str) -> [u8; 16] {
        let hex: Vec<u8> = text
            .split('-')
            .collect::<String>()
            .as_bytes()
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        let mut bytes = [0; 16];
        for (index, place) in [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15]
            .into_iter()
            .enumerate()
        {
            bytes[index] = hex[place];
        }
        bytes
    }

    /// The first MiB of an image with this table in it, built the way the specification lays it out.
    fn image_start(table: &Table) -> Vec<u8> {
        let sector = 512;
        let mut bytes = vec![0; GPT_BYTES];
        let mut entries = vec![0_u8; 128 * 128];
        for (index, partition) in table.partitions.iter().enumerate() {
            let entry = &mut entries[index * 128..(index + 1) * 128];
            entry[..16].copy_from_slice(&guid_bytes(&partition.kind));
            entry[16..32].copy_from_slice(&guid_bytes(partition.uuid.as_deref().unwrap()));
            entry[32..40].copy_from_slice(&partition.start.to_le_bytes());
            entry[40..48].copy_from_slice(&(partition.start + partition.size - 1).to_le_bytes());
            if partition.attrs.as_deref() == Some("GUID:60") {
                entry[48..56].copy_from_slice(&(1_u64 << 60).to_le_bytes());
            }
            for (at, unit) in partition
                .name
                .as_deref()
                .unwrap()
                .encode_utf16()
                .enumerate()
            {
                entry[56 + 2 * at..58 + 2 * at].copy_from_slice(&unit.to_le_bytes());
            }
        }
        bytes[2 * sector..2 * sector + entries.len()].copy_from_slice(&entries);
        let header = &mut bytes[sector..sector + 92];
        header[..8].copy_from_slice(SIGNATURE);
        header[8..12].copy_from_slice(&0x0001_0000_u32.to_le_bytes());
        header[12..16].copy_from_slice(&92_u32.to_le_bytes());
        header[24..32].copy_from_slice(&1_u64.to_le_bytes());
        header[32..40].copy_from_slice(&16_201_215_u64.to_le_bytes());
        header[40..48].copy_from_slice(&2048_u64.to_le_bytes());
        header[48..56].copy_from_slice(&16_201_182_u64.to_le_bytes());
        header[56..72].copy_from_slice(&guid_bytes("B581FEF7-24ED-4F31-990B-099EC86BBA03"));
        header[72..80].copy_from_slice(&2_u64.to_le_bytes());
        header[80..84].copy_from_slice(&128_u32.to_le_bytes());
        header[84..88].copy_from_slice(&128_u32.to_le_bytes());
        header[88..92].copy_from_slice(&crc32(&entries).to_le_bytes());
        let sum = crc32(header);
        header[16..20].copy_from_slice(&sum.to_le_bytes());
        bytes
    }

    #[test]
    fn the_checksum_is_the_usual_crc32() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn guids_keep_the_byte_order_of_the_specification() {
        // the esp's type as it is stored on every disk
        let stored = [
            0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E,
            0xC9, 0x3B,
        ];
        assert_eq!(guid(&stored), "C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
        assert_eq!(guid_bytes("C12A7328-F81F-11D2-BA4B-00A0C93EC93B"), stored);
    }

    #[test]
    fn attributes_are_written_the_way_sfdisk_writes_them() {
        assert_eq!(attributes(0), None);
        assert_eq!(attributes(1 << 60).as_deref(), Some("GUID:60"));
        assert_eq!(
            attributes(1 | (1 << 2) | (1 << 48) | (1 << 63)).as_deref(),
            Some("RequiredPartition LegacyBIOSBootable GUID:48,63")
        );
    }

    #[test]
    fn the_image_table_reads_as_sfdisk_reads_it() {
        let expected = read_table(IMAGE).unwrap();
        let start = image_start(&expected);
        assert_eq!(read_gpt(&start), Ok(expected));
    }

    #[test]
    fn damaged_or_missing_tables_are_refused() {
        let table = read_table(IMAGE).unwrap();
        let good = image_start(&table);
        assert_eq!(
            read_gpt(&vec![0; GPT_BYTES]),
            Err("It has no GUID partition table.".into())
        );
        let mut header = good.clone();
        header[512 + 40] ^= 1;
        assert_eq!(
            read_gpt(&header),
            Err("Its partition table header is damaged.".into())
        );
        let mut entry = good.clone();
        entry[1024 + 128 + 60] ^= 1;
        assert_eq!(
            read_gpt(&entry),
            Err("Its partition entries are damaged.".into())
        );
        assert!(read_gpt(&good[..1000]).is_err());
    }
}
