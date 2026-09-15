//! The system image rift-flash writes: its partition table and the slot in it, and its bytes from
//! start to end, out of a raw file or one compressed with zstd.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use librift::disk::{
    ESP_SIZE, ESP_TYPE, GPT_BYTES, Partition, SECTOR, STORE_SIZE, Slot, Table, USR_TYPE,
    USR_VERITY_TYPE, VERITY_SIZE, read_gpt, size,
};
use ruzstd::decoding::{FrameDecoder, StreamingDecoder};

/// How every zstd frame starts.
const ZSTD_FRAME: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// A system image and what is in it.
#[derive(Debug, Clone)]
pub struct Image {
    /// The file.
    pub path: PathBuf,
    /// Its partition table.
    pub table: Table,
    /// Its esp.
    pub esp: Partition,
    /// Its slot: the version, the verity partition and the store.
    pub slot: Slot,
}

impl Image {
    /// Reads the partition table at the start of the image at `path` and finds the slot in it.
    pub fn open(path: &Path) -> Result<Image, String> {
        let shown = path.display();
        let mut reader = Reader::open(path)?;
        let mut start = vec![0; GPT_BYTES];
        reader
            .read_exact(&mut start)
            .map_err(|e| format!("Could not read {shown}: {e}"))?;
        let refused = |why: String| format!("{shown} is not a Rift image. {why}");
        let table = read_gpt(&start).map_err(refused)?;
        let (esp, slot) = layout(&table).map_err(refused)?;
        if !reader.compressed() {
            let end = table.offset(&slot.store) + table.bytes(&slot.store);
            let length = fs::metadata(path)
                .map_err(|e| format!("Could not read {shown}: {e}"))?
                .len();
            if length < end {
                return Err(format!(
                    "{shown} is cut off: it has {}, and its store ends at {}.",
                    size(length),
                    size(end)
                ));
            }
        }
        Ok(Image {
            path: path.to_path_buf(),
            table,
            esp,
            slot,
        })
    }

    /// The image's bytes from its start.
    pub fn reader(&self) -> Result<Reader, String> {
        Reader::open(&self.path)
    }

    /// A read back of what was copied from the image, which reads the image again from its start.
    pub fn check(&self) -> Result<Check<'_>, String> {
        Ok(Check {
            image: self,
            reader: self.reader()?,
        })
    }
}

/// How much is compared at a time.
const CHUNK: usize = 4 << 20;

/// Reads what was copied from an image back from the drive and compares it with the image. A stick
/// that holds less than it says wraps around or drops what is written past its end, and then the
/// bytes that read back are not the ones written. Found here, it is not found at boot by dm-verity.
pub struct Check<'a> {
    image: &'a Image,
    reader: Reader,
}

impl Check<'_> {
    /// Compares the image's partition `from` with what `drive`, which reads `to`, holds from `offset`
    /// on, saying how far it is at each tenth. Partitions are compared in the order of the image.
    pub fn partition(
        &mut self,
        from: &Partition,
        drive: &mut (impl Read + Seek),
        (to, offset): (&Path, u64),
        say: &mut dyn FnMut(String),
    ) -> Result<(), String> {
        let image = self.image;
        let reading = |e: io::Error| format!("Could not read {}: {e}", image.path.display());
        let back = |e: io::Error| format!("Could not read {} back: {e}", to.display());
        self.reader
            .skip_to(image.table.offset(from))
            .map_err(reading)?;
        drive.seek(SeekFrom::Start(offset)).map_err(back)?;
        let bytes = image.table.bytes(from);
        let mut wanted = vec![0; CHUNK];
        let mut found = vec![0; CHUNK];
        let mut done = 0;
        let mut tenth = 1;
        while done < bytes {
            let count = usize::try_from(bytes - done).map_or(CHUNK, |left| left.min(CHUNK));
            self.reader
                .read_exact(&mut wanted[..count])
                .map_err(reading)?;
            drive.read_exact(&mut found[..count]).map_err(back)?;
            if let Some(at) = wanted[..count]
                .iter()
                .zip(&found[..count])
                .position(|(written, read)| written != read)
            {
                return Err(format!(
                    "{} does not read back what was written at {}. The disk holds less than it says it does, or it is failing.",
                    to.display(),
                    size(offset + done + u64::try_from(at).unwrap_or(0))
                ));
            }
            done += u64::try_from(count).unwrap_or(u64::MAX);
            if tenth < 10 && done * 10 >= bytes * tenth {
                say(format!("Checked {} of {}.", size(done), size(bytes)));
                while tenth < 10 && done * 10 >= bytes * tenth {
                    tenth += 1;
                }
            }
        }
        Ok(())
    }
}

/// The esp and the slot in an image's table. An image is exactly an esp, a verity partition and a
/// store, in that order, each no bigger than its partition on a drive.
pub fn layout(table: &Table) -> Result<(Partition, Slot), String> {
    if table.sectorsize != SECTOR {
        return Err(format!(
            "Its sectors have {} bytes, not {SECTOR}.",
            table.sectorsize
        ));
    }
    let [esp, verity, store] = table.partitions.as_slice() else {
        return Err(format!(
            "It has {} partitions, not an esp, a verity partition and a store.",
            table.partitions.len()
        ));
    };
    if esp.kind != ESP_TYPE || verity.kind != USR_VERITY_TYPE || store.kind != USR_TYPE {
        return Err(
            "Its partitions are not an esp, a verity partition and a store, in that order.".into(),
        );
    }
    let label = |partition: &Partition| partition.name.clone().unwrap_or_else(|| "nothing".into());
    let version = store
        .name
        .as_deref()
        .and_then(|name| name.strip_prefix("store_"))
        .filter(|version| {
            !version.is_empty()
                && version
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
        .ok_or_else(|| {
            format!(
                "Its store is labelled {}, not store_ and a version.",
                label(store)
            )
        })?;
    let wanted = format!("store-verity_{version}");
    if verity.name.as_deref() != Some(wanted.as_str()) {
        return Err(format!(
            "Its verity partition is labelled {}, not {wanted}.",
            label(verity)
        ));
    }
    if verity.uuid.is_none() || store.uuid.is_none() {
        return Err("Its verity partition or its store has no uuid.".into());
    }
    for (partition, what, most) in [
        (esp, "esp", ESP_SIZE),
        (verity, "verity partition", VERITY_SIZE),
        (store, "store", STORE_SIZE),
    ] {
        if table.bytes(partition) > most {
            return Err(format!(
                "Its {what} has {}, more than the {} a drive has for it.",
                size(table.bytes(partition)),
                size(most)
            ));
        }
    }
    if esp.start + esp.size > verity.start || verity.start + verity.size > store.start {
        return Err("Its partitions overlap.".into());
    }
    Ok((
        esp.clone(),
        Slot {
            version: version.to_string(),
            verity: verity.clone(),
            store: store.clone(),
        },
    ))
}

/// An image's bytes from its start, uncompressed, read once from start to end.
pub struct Reader {
    source: Source,
    at: u64,
}

enum Source {
    Raw(File),
    Zstd(Box<Frames>),
}

impl Reader {
    fn open(path: &Path) -> Result<Reader, String> {
        let reading = |e: io::Error| format!("Could not read {}: {e}", path.display());
        let mut file = File::open(path).map_err(reading)?;
        let mut first = [0; 4];
        let compressed = file.read_exact(&mut first).is_ok() && first == ZSTD_FRAME;
        file.seek(SeekFrom::Start(0)).map_err(reading)?;
        let source = if compressed {
            let decoder = StreamingDecoder::new(BufReader::with_capacity(1 << 20, file))
                .map_err(|e| format!("Could not read the zstd frame in {}: {e}", path.display()))?;
            Source::Zstd(Box::new(Frames {
                decoder: Some(decoder),
            }))
        } else {
            Source::Raw(file)
        };
        Ok(Reader { source, at: 0 })
    }

    /// Whether the image is compressed with zstd.
    pub fn compressed(&self) -> bool {
        matches!(self.source, Source::Zstd(_))
    }

    /// Skips ahead to `offset`. A compressed image is read once, so there is no going back.
    pub fn skip_to(&mut self, offset: u64) -> io::Result<()> {
        let ahead = offset
            .checked_sub(self.at)
            .ok_or_else(|| io::Error::other("the image is read from its start to its end"))?;
        if let Source::Raw(file) = &mut self.source {
            file.seek(SeekFrom::Start(offset))?;
            self.at = offset;
            return Ok(());
        }
        let skipped = io::copy(&mut Read::by_ref(self).take(ahead), &mut io::sink())?;
        if skipped < ahead {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(())
    }

    /// Reads what is left of a compressed image, so the checksum at its end is compared.
    pub fn finish(&mut self) -> io::Result<()> {
        if self.compressed() {
            io::copy(self, &mut io::sink())?;
        }
        Ok(())
    }
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let got = match &mut self.source {
            Source::Raw(file) => file.read(buf)?,
            Source::Zstd(frames) => frames.read(buf)?,
        };
        self.at += u64::try_from(got).unwrap_or(u64::MAX);
        Ok(got)
    }
}

/// The frames of a zstd file one after another, each compared with its checksum when it has one.
struct Frames {
    decoder: Option<StreamingDecoder<BufReader<File>, FrameDecoder>>,
}

impl Read for Frames {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while let Some(decoder) = self.decoder.as_mut() {
            let got = decoder.read(buf)?;
            if got > 0 || buf.is_empty() {
                return Ok(got);
            }
            let frame = &decoder.decoder;
            if let (Some(stored), Some(counted)) = (
                frame.get_checksum_from_data(),
                frame.get_calculated_checksum(),
            ) && stored != counted
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the checksum of its zstd frame does not match, the image is damaged",
                ));
            }
            let Some((mut source, _)) = self.decoder.take().map(StreamingDecoder::into_parts)
            else {
                break;
            };
            if source.fill_buf()?.is_empty() {
                return Ok(0);
            }
            self.decoder = Some(
                StreamingDecoder::new(source)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?,
            );
        }
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use librift::disk::{MIB, read_table};
    use ruzstd::encoding::{CompressionLevel, compress_to_vec};

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

    #[test]
    fn the_image_the_flake_builds_has_a_slot() {
        let table = read_table(IMAGE).unwrap();
        let (esp, slot) = layout(&table).unwrap();
        assert_eq!(esp.name.as_deref(), Some("esp"));
        assert_eq!(slot.version, "0.1.0");
        assert_eq!(
            slot.store.uuid.as_deref(),
            Some("229D80B9-7A5A-031B-79F2-324122B3A4F0")
        );
    }

    #[test]
    fn tables_that_are_not_an_image_are_refused() {
        let image = read_table(IMAGE).unwrap();
        let refused = |change: &dyn Fn(&mut Table), words: &str| {
            let mut table = image.clone();
            change(&mut table);
            let why = layout(&table).unwrap_err();
            assert!(why.contains(words), "{why}");
        };
        refused(&|table| table.sectorsize = 4096, "sectors have 4096 bytes");
        refused(
            &|table| {
                table.partitions.pop();
            },
            "It has 2 partitions",
        );
        refused(
            &|table| table.partitions.swap(1, 2),
            "not an esp, a verity partition and a store",
        );
        refused(
            &|table| table.partitions[2].name = Some("store_0 1".into()),
            "labelled store_0 1, not store_ and a version",
        );
        refused(&|table| table.partitions[2].name = None, "labelled nothing");
        refused(
            &|table| table.partitions[1].name = Some("store-verity_0.2.0".into()),
            "not store-verity_0.1.0",
        );
        refused(&|table| table.partitions[1].uuid = None, "has no uuid");
        refused(
            &|table| table.partitions[2].size = 16_777_217,
            "Its store has 8.0 GiB, more than the 8.0 GiB",
        );
        refused(
            &|table| table.partitions[1].start = 2048,
            "Its partitions overlap.",
        );
    }

    fn folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("rift-flash-{name}-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// A first MiB of zeros, the way an image starts before its table, then bytes that change.
    fn pattern(bytes: usize) -> Vec<u8> {
        let mib = usize::try_from(MIB).unwrap();
        (0..bytes)
            .map(|at| {
                if at < mib {
                    0
                } else {
                    u8::try_from(at % 251).unwrap()
                }
            })
            .collect()
    }

    #[test]
    fn a_compressed_image_reads_like_the_raw_one() {
        let folder = folder("reader");
        let data = pattern(6 << 20);
        let raw = folder.join("image.raw");
        fs::write(&raw, &data).unwrap();
        // two frames, the way a parallel compressor writes one file
        let mut packed = compress_to_vec(&data[..4 << 20], CompressionLevel::Uncompressed);
        packed.extend(compress_to_vec(
            &data[4 << 20..],
            CompressionLevel::Uncompressed,
        ));
        let zst = folder.join("image.raw.zst");
        fs::write(&zst, &packed).unwrap();

        for (path, compressed) in [(&raw, false), (&zst, true)] {
            let mut reader = Reader::open(path).unwrap();
            assert_eq!(reader.compressed(), compressed);
            reader.skip_to(MIB + 3).unwrap();
            let mut chunk = vec![0; 100];
            reader.read_exact(&mut chunk).unwrap();
            assert_eq!(chunk, &data[(1 << 20) + 3..(1 << 20) + 103]);
            // across the end of the first frame
            reader.skip_to((4 << 20) - 10).unwrap();
            let mut across = vec![0; 20];
            reader.read_exact(&mut across).unwrap();
            assert_eq!(across, &data[(4 << 20) - 10..(4 << 20) + 10]);
            assert!(reader.skip_to(MIB).is_err(), "{path:?} went back");
            reader.finish().unwrap();
        }

        // a compressed image cut off in its second frame
        let cut = folder.join("cut.raw.zst");
        fs::write(&cut, &packed[..packed.len() - 1000]).unwrap();
        let mut reader = Reader::open(&cut).unwrap();
        assert!(reader.skip_to(6 << 20).is_err());
        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_file_that_is_not_an_image_is_refused_when_it_is_opened() {
        let folder = folder("open");
        let empty = folder.join("empty.raw");
        fs::write(&empty, vec![0; 2 << 20]).unwrap();
        let why = Image::open(&empty).unwrap_err();
        assert!(
            why.ends_with("empty.raw is not a Rift image. It has no GUID partition table."),
            "{why}"
        );
        let short = folder.join("short.raw");
        fs::write(&short, [0; 100]).unwrap();
        assert!(
            Image::open(&short)
                .unwrap_err()
                .starts_with("Could not read")
        );
        fs::remove_dir_all(&folder).unwrap();
    }
}
