//! GUID partition tables without a program to ask: reading one out of the first bytes of a disk
//! image, and writing one for a disk on systems that have no sfdisk. rift-flash reads an image's
//! table this way, out of a compressed image too, and writes a drive's table this way on macOS and
//! Windows.

use std::fmt::Write as _;

use super::table::{ALIGN, Partition, Table};

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
/// The entries a written table has room for, and the bytes of each, as sfdisk writes them.
const ENTRIES: usize = 128;
const ENTRY_BYTES: usize = 128;
/// The sectors those entries take.
const ENTRY_SECTORS: u64 = 32;
/// Where the stored bytes of a GUID come from in its text: the first three fields are little endian.
const GUID_ORDER: [usize; 16] = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];

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
        id: Some(guid(&header[56..72])),
        sectorsize: u64::try_from(sector).unwrap_or(512),
        partitions,
    })
}

/// A partition table as the bytes on a disk: the protective MBR, the header and the entries at its
/// start, the entries again and the backup header at its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gpt {
    /// The first 34 sectors.
    pub primary: Vec<u8>,
    /// The last 33 sectors.
    pub backup: Vec<u8>,
    /// Where the last 33 sectors start, in bytes.
    pub backup_offset: u64,
}

/// The bytes of `table` on a disk of `sectors` sectors of 512 bytes, laid out the way sfdisk writes
/// a new table: 128 entries, partitions from 1 MiB on, the disk's GUID from the table's id.
///
/// # Errors
///
/// When the table has no id, a type or uuid that is not a GUID, a name of more than 36 characters,
/// attributes sfdisk would not write, more partitions than entries, or partitions that do not fit or
/// overlap.
pub fn write_gpt(table: &Table, sectors: u64) -> Result<Gpt, String> {
    if table.sectorsize != 512 {
        return Err(format!(
            "A table of sectors of {} bytes cannot be written.",
            table.sectorsize
        ));
    }
    let disk_guid = table
        .id
        .as_deref()
        .and_then(guid_bytes)
        .ok_or("The partition table has no GUID of its own.")?;
    let last_usable = sectors
        .checked_sub(2 + ENTRY_SECTORS)
        .filter(|&last| last >= ALIGN)
        .ok_or_else(|| format!("A disk of {sectors} sectors has no room for a partition table."))?;
    if table.partitions.len() > ENTRIES {
        return Err(format!(
            "A partition table has room for {ENTRIES} partitions, not {}.",
            table.partitions.len()
        ));
    }

    let entries = entries(table, last_usable)?;

    let last = sectors - 1;
    let entries_sum = crc32(&entries);
    let header = |mine: u64, other: u64, entries_at: u64| {
        let mut bytes = vec![0_u8; 512];
        bytes[..8].copy_from_slice(SIGNATURE);
        bytes[8..12].copy_from_slice(&0x0001_0000_u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&92_u32.to_le_bytes());
        bytes[24..32].copy_from_slice(&mine.to_le_bytes());
        bytes[32..40].copy_from_slice(&other.to_le_bytes());
        bytes[40..48].copy_from_slice(&ALIGN.to_le_bytes());
        bytes[48..56].copy_from_slice(&last_usable.to_le_bytes());
        bytes[56..72].copy_from_slice(&disk_guid);
        bytes[72..80].copy_from_slice(&entries_at.to_le_bytes());
        bytes[80..84].copy_from_slice(&128_u32.to_le_bytes());
        bytes[84..88].copy_from_slice(&128_u32.to_le_bytes());
        bytes[88..92].copy_from_slice(&entries_sum.to_le_bytes());
        let sum = crc32(&bytes[..92]);
        bytes[16..20].copy_from_slice(&sum.to_le_bytes());
        bytes
    };

    let mut primary = vec![0_u8; 512];
    // one partition of type 0xEE over the whole disk, so tools that only know MBR leave it alone
    let record = &mut primary[446..462];
    record[2] = 0x02;
    record[4] = 0xEE;
    record[5..8].fill(0xFF);
    record[8..12].copy_from_slice(&1_u32.to_le_bytes());
    record[12..16].copy_from_slice(&u32::try_from(last).unwrap_or(u32::MAX).to_le_bytes());
    primary[510] = 0x55;
    primary[511] = 0xAA;
    primary.extend(header(1, last, 2));
    primary.extend(&entries);

    let entries_at = last - ENTRY_SECTORS;
    let mut backup = entries;
    backup.extend(header(last, 1, entries_at));
    Ok(Gpt {
        primary,
        backup,
        backup_offset: entries_at * 512,
    })
}

/// The partition entries of `table`, each checked to lie between 1 MiB and `last_usable` and not
/// to overlap another.
fn entries(table: &Table, last_usable: u64) -> Result<Vec<u8>, String> {
    let mut entries = vec![0_u8; ENTRIES * ENTRY_BYTES];
    let mut taken: Vec<(u64, u64)> = Vec::new();
    for (index, partition) in table.partitions.iter().enumerate() {
        let number = index + 1;
        let end = (partition.start + partition.size)
            .checked_sub(1)
            .filter(|_| partition.size > 0)
            .ok_or_else(|| format!("Partition {number} is empty."))?;
        if partition.start < ALIGN || end > last_usable {
            return Err(format!("Partition {number} does not fit on the disk."));
        }
        if taken
            .iter()
            .any(|&(start, last)| partition.start <= last && start <= end)
        {
            return Err(format!("Partition {number} overlaps another."));
        }
        taken.push((partition.start, end));
        let entry = &mut entries[index * ENTRY_BYTES..number * ENTRY_BYTES];
        entry[..16].copy_from_slice(
            &guid_bytes(&partition.kind)
                .ok_or_else(|| format!("The type of partition {number} is not a GUID."))?,
        );
        entry[16..32].copy_from_slice(
            &partition
                .uuid
                .as_deref()
                .and_then(guid_bytes)
                .ok_or_else(|| format!("Partition {number} has no uuid."))?,
        );
        entry[32..40].copy_from_slice(&partition.start.to_le_bytes());
        entry[40..48].copy_from_slice(&end.to_le_bytes());
        let bits = partition
            .attrs
            .as_deref()
            .map_or(Some(0), attribute_bits)
            .ok_or_else(|| format!("The attributes of partition {number} cannot be written."))?;
        entry[48..56].copy_from_slice(&bits.to_le_bytes());
        let units: Vec<u16> = partition
            .name
            .as_deref()
            .unwrap_or("")
            .encode_utf16()
            .collect();
        if units.len() > 36 {
            return Err(format!("The name of partition {number} is too long."));
        }
        for (at, unit) in units.into_iter().enumerate() {
            entry[56 + 2 * at..58 + 2 * at].copy_from_slice(&unit.to_le_bytes());
        }
    }
    Ok(entries)
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
    for (place, index) in GUID_ORDER.into_iter().enumerate() {
        if matches!(place, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        let _ = write!(text, "{:02X}", bytes[index]);
    }
    text
}

/// A GUID in text as the bytes it is stored as.
fn guid_bytes(text: &str) -> Option<[u8; 16]> {
    let dashes = [8, 13, 18, 23];
    let valid = text.len() == 36
        && text.bytes().enumerate().all(|(at, byte)| {
            if dashes.contains(&at) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
    if !valid {
        return None;
    }
    let digits: Vec<u8> = text
        .bytes()
        .filter(|&byte| byte != b'-')
        .map(|byte| match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => byte - b'A' + 10,
        })
        .collect();
    let mut stored = [0; 16];
    for (index, place) in GUID_ORDER.into_iter().enumerate() {
        stored[index] = (digits[2 * place] << 4) | digits[2 * place + 1];
    }
    Some(stored)
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

/// The attribute bits in words the way sfdisk writes them, the other way from [`attributes`].
fn attribute_bits(text: &str) -> Option<u64> {
    let mut bits = 0;
    for word in text.split_whitespace() {
        if let Some(numbers) = word.strip_prefix("GUID:") {
            for number in numbers.split(',') {
                let bit = number
                    .parse::<u32>()
                    .ok()
                    .filter(|bit| (48..64).contains(bit))?;
                bits |= 1 << bit;
            }
        } else {
            let (bit, _) = NAMED_BITS.iter().find(|(_, name)| *name == word)?;
            bits |= 1 << bit;
        }
    }
    Some(bits)
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

    /// What sfdisk 2.42 printed for `rift_0.1.0.raw`, the image the flake builds.
    const IMAGE: &str = r#"{
       "partitiontable": {
          "label": "gpt", "id": "B581FEF7-24ED-4F31-990B-099EC86BBA03", "device": "result/rift_0.1.0.raw",
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

    /// Sectors in a disk the image's table fits on, with nothing to spare.
    const SECTORS: u64 = 16_201_216 + 33;

    /// The first MiB of an image with this table in it, built the way the specification lays it out.
    fn image_start(table: &Table) -> Vec<u8> {
        let sector = 512;
        let mut bytes = vec![0; GPT_BYTES];
        let mut entries = vec![0_u8; 128 * 128];
        for (index, partition) in table.partitions.iter().enumerate() {
            let entry = &mut entries[index * 128..(index + 1) * 128];
            entry[..16].copy_from_slice(&guid_bytes(&partition.kind).unwrap());
            entry[16..32].copy_from_slice(&guid_bytes(partition.uuid.as_deref().unwrap()).unwrap());
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
        header[56..72]
            .copy_from_slice(&guid_bytes("B581FEF7-24ED-4F31-990B-099EC86BBA03").unwrap());
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
        assert_eq!(
            guid_bytes("C12A7328-F81F-11D2-BA4B-00A0C93EC93B"),
            Some(stored)
        );
        assert_eq!(
            guid_bytes("c12a7328-f81f-11d2-ba4b-00a0c93ec93b"),
            Some(stored)
        );
        for wrong in [
            "",
            "C12A7328F81F11D2BA4B00A0C93EC93B",
            "C12A7328-F81F-11D2-BA4B-00A0C93EC93",
            "C12A7328-F81F-11D2-BA4B-00A0C93EC93G",
            "C12A7328+F81F-11D2-BA4B-00A0C93EC93B",
        ] {
            assert_eq!(guid_bytes(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn attributes_are_written_the_way_sfdisk_writes_them() {
        assert_eq!(attributes(0), None);
        assert_eq!(attributes(1 << 60).as_deref(), Some("GUID:60"));
        let many = 1 | (1 << 2) | (1 << 48) | (1 << 63);
        assert_eq!(
            attributes(many).as_deref(),
            Some("RequiredPartition LegacyBIOSBootable GUID:48,63")
        );
        assert_eq!(
            attribute_bits("RequiredPartition LegacyBIOSBootable GUID:48,63"),
            Some(many)
        );
        assert_eq!(attribute_bits("GUID:60"), Some(1 << 60));
        assert_eq!(attribute_bits(""), Some(0));
        assert_eq!(attribute_bits("GUID:47"), None);
        assert_eq!(attribute_bits("Hidden"), None);
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

    #[test]
    fn a_written_table_reads_back_as_the_table() {
        let table = read_table(IMAGE).unwrap();
        let gpt = write_gpt(&table, SECTORS).unwrap();
        assert_eq!(gpt.primary.len(), 34 * 512);
        assert_eq!(gpt.backup.len(), 33 * 512);
        assert_eq!(gpt.backup_offset, (SECTORS - 33) * 512);
        let mut start = gpt.primary.clone();
        start.resize(GPT_BYTES, 0);
        assert_eq!(read_gpt(&start), Ok(table));

        // the protective MBR: one partition of type 0xEE from sector 1 to the end
        assert_eq!(
            &gpt.primary[446..462],
            &[
                0, 0, 2, 0, 0xEE, 0xFF, 0xFF, 0xFF, 1, 0, 0, 0, 0x20, 0x36, 0xF7, 0
            ]
        );
        assert_eq!(&gpt.primary[510..512], &[0x55, 0xAA]);

        // the backup: the same entries, then a header that points back at the first one
        assert_eq!(&gpt.backup[..128 * 128], &gpt.primary[1024..]);
        let backup = &gpt.backup[32 * 512..];
        let primary = &gpt.primary[512..1024];
        assert_eq!(&backup[..8], SIGNATURE);
        assert_eq!(u64_at(backup, 24), SECTORS - 1);
        assert_eq!(u64_at(backup, 32), 1);
        assert_eq!(u64_at(primary, 32), SECTORS - 1);
        assert_eq!(u64_at(backup, 72), SECTORS - 33);
        assert_eq!(u64_at(backup, 48), SECTORS - 34);
        // the same disk guid, entry count, entry size and entry checksum; the entries lie elsewhere
        assert_eq!(&backup[56..72], &primary[56..72]);
        assert_eq!(&backup[80..92], &primary[80..92]);
        let mut blank = backup[..92].to_vec();
        blank[16..20].fill(0);
        assert_eq!(crc32(&blank), u32_at(backup, 16));
        assert!(backup[92..].iter().all(|&byte| byte == 0));
    }

    /// What sfdisk 2.39.3 wrote for this table into a 24G file, by the checksums of its first 34
    /// sectors and its last 33.
    #[test]
    fn a_table_is_written_byte_for_byte_as_sfdisk_writes_it() {
        let table = read_table(
            r#"{"partitiontable": {"id": "05050505-0505-4505-8505-050505050505", "partitions": [
            {"node": "1", "start": 2048, "size": 2097152, "type": "C12A7328-F81F-11D2-BA4B-00A0C93EC93B",
             "uuid": "01010101-0101-4101-8101-010101010101", "name": "esp"},
            {"node": "2", "start": 2099200, "size": 2097152, "type": "77FF5F63-E7B6-4633-ACF4-1565B864C0E6",
             "uuid": "5C1D8E0A-2B3C-4D5E-8F90-A1B2C3D4E5F6", "name": "store-verity_0.2.0", "attrs": "GUID:60"},
            {"node": "3", "start": 4196352, "size": 16777216, "type": "8484680C-9521-48C6-9C11-B0720656F69E",
             "uuid": "7E8F9A0B-1C2D-4E3F-9051-627384950A1B", "name": "store_0.2.0"},
            {"node": "4", "start": 20973568, "size": 2097152, "type": "77FF5F63-E7B6-4633-ACF4-1565B864C0E6",
             "uuid": "02020202-0202-4202-8202-020202020202", "name": "_empty"},
            {"node": "5", "start": 23070720, "size": 16777216, "type": "8484680C-9521-48C6-9C11-B0720656F69E",
             "uuid": "03030303-0303-4303-8303-030303030303", "name": "_empty"},
            {"node": "6", "start": 39847936, "size": 2097152, "type": "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7",
             "uuid": "04040404-0404-4404-8404-040404040404", "name": "exchange"}
            ]}}"#,
        )
        .unwrap();
        let gpt = write_gpt(&table, 24 << 21).unwrap();
        assert_eq!(crc32(&gpt.primary), 0x3B76_C722);
        assert_eq!(crc32(&gpt.backup), 0x99B2_D9BC);
    }

    #[test]
    fn tables_that_cannot_be_written_are_refused() {
        let table = read_table(IMAGE).unwrap();
        let refused = |change: &dyn Fn(&mut Table), sectors: u64, words: &str| {
            let mut changed = table.clone();
            change(&mut changed);
            let why = write_gpt(&changed, sectors).unwrap_err();
            assert!(why.contains(words), "{why}");
        };
        refused(&|_| {}, SECTORS - 1, "Partition 3 does not fit");
        refused(&|_| {}, 2000, "has no room for a partition table");
        refused(&|table| table.id = None, SECTORS, "no GUID of its own");
        refused(
            &|table| table.sectorsize = 4096,
            SECTORS,
            "sectors of 4096 bytes",
        );
        refused(
            &|table| table.partitions[1].start = 2049,
            SECTORS,
            "Partition 2 overlaps",
        );
        refused(
            &|table| table.partitions[0].start = 34,
            SECTORS,
            "Partition 1 does not fit",
        );
        refused(
            &|table| table.partitions[2].size = 0,
            SECTORS,
            "Partition 3 is empty",
        );
        refused(
            &|table| table.partitions[1].uuid = None,
            SECTORS,
            "Partition 2 has no uuid",
        );
        refused(
            &|table| table.partitions[0].kind = "esp".into(),
            SECTORS,
            "type of partition 1 is not a GUID",
        );
        refused(
            &|table| table.partitions[0].name = Some("x".repeat(37)),
            SECTORS,
            "name of partition 1 is too long",
        );
        refused(
            &|table| table.partitions[0].attrs = Some("Hidden".into()),
            SECTORS,
            "attributes of partition 1",
        );
        let mut full = table.clone();
        full.partitions = (0..129).map(|_| table.partitions[0].clone()).collect();
        assert!(
            write_gpt(&full, SECTORS)
                .unwrap_err()
                .contains("room for 128")
        );
        // a name of 36 characters is the most there is room for
        let mut long = table;
        long.partitions[0].name = Some("x".repeat(36));
        assert!(write_gpt(&long, SECTORS).is_ok());
    }
}
