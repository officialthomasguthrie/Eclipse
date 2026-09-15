//! What rift-flash checks without touching a disk: sizes typed in, the folder of models, whether
//! a file is empty, and whether the table sfdisk wrote is the one asked for.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use librift::disk::{GIB, GPT_BYTES, MIB, Slot, Table, size};

/// Refuses a file a drive must not be written into: one that is too small, or holds anything at its
/// start. Returns its size.
pub fn check_file(path: &Path, need: u64) -> Result<u64, String> {
    let shown = path.display();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("Could not open {shown} to write it: {e}"))?;
    let bytes = file
        .metadata()
        .map_err(|e| format!("Could not look at {shown}: {e}"))?
        .len();
    if bytes < need {
        return Err(format!(
            "{shown} holds {}, and Rift needs {}.",
            size(bytes),
            size(need)
        ));
    }
    let mut start = vec![0; GPT_BYTES];
    file.read_exact(&mut start)
        .map_err(|e| format!("Could not read {shown}: {e}"))?;
    if !is_empty(&start) {
        return Err(format!(
            "{shown} is not empty, and rift-flash only writes into an empty file. Make one with {MAKE_FILE}."
        ));
    }
    Ok(bytes)
}

/// How a person makes an empty file of 24G for a drive on this system.
#[cfg(target_os = "linux")]
const MAKE_FILE: &str = "truncate -s 24G <file>";
#[cfg(target_os = "macos")]
const MAKE_FILE: &str = "mkfile -n 24g <file>";
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const MAKE_FILE: &str = "fsutil file createnew <file> 25769803776";

/// The least an exchange partition gets.
const LEAST_EXCHANGE: u64 = 64 * MIB;

/// The size of the exchange partition in `--exchange`: `8G` or `512M`, in binary units, or `0` and
/// `none` for no exchange partition.
pub fn exchange_size(text: &str) -> Result<Option<u64>, String> {
    let lower = text.trim().to_ascii_lowercase();
    if lower == "0" || lower == "none" {
        return Ok(None);
    }
    let (number, unit) = if let Some(number) = lower
        .strip_suffix("gib")
        .or_else(|| lower.strip_suffix('g'))
    {
        (number, GIB)
    } else if let Some(number) = lower
        .strip_suffix("mib")
        .or_else(|| lower.strip_suffix('m'))
    {
        (number, MIB)
    } else {
        return Err(format!(
            "the exchange size {text} has no unit, give it in G or M, like 8G"
        ));
    };
    let bytes = number
        .parse::<u64>()
        .ok()
        .and_then(|count| count.checked_mul(unit))
        .ok_or_else(|| format!("{text} is not a size, give it in G or M, like 8G"))?;
    if bytes == 0 {
        return Ok(None);
    }
    if bytes < LEAST_EXCHANGE {
        return Err(format!(
            "an exchange partition needs at least {}",
            size(LEAST_EXCHANGE)
        ));
    }
    Ok(Some(bytes))
}

/// The files in a folder of models, by name, and the bytes they hold together.
pub fn models_in(folder: &Path) -> Result<(Vec<PathBuf>, u64), String> {
    let reading =
        |e: std::io::Error| format!("Could not read the folder {}: {e}", folder.display());
    let mut files = Vec::new();
    let mut bytes = 0;
    for entry in fs::read_dir(folder).map_err(reading)? {
        let path = entry.map_err(reading)?.path();
        let meta = fs::metadata(&path).map_err(reading)?;
        if meta.is_file() {
            bytes += meta.len();
            files.push(path);
        }
    }
    if files.is_empty() {
        return Err(format!("{} holds no files.", folder.display()));
    }
    files.sort();
    Ok((files, bytes))
}

/// Whether the start of a file holds nothing but zeros.
pub fn is_empty(start: &[u8]) -> bool {
    start.iter().all(|&byte| byte == 0)
}

/// The labels of a new drive's partitions, in order.
pub fn labels(slot: &Slot, exchange: bool) -> Vec<String> {
    let mut labels = vec![
        "esp".to_string(),
        format!("store-verity_{}", slot.version),
        format!("store_{}", slot.version),
        "_empty".to_string(),
        "_empty".to_string(),
    ];
    if exchange {
        labels.push("exchange".to_string());
    }
    labels.push("persist".to_string());
    labels
}

/// Checks that the table sfdisk wrote has the partitions that were asked for, in that order.
pub fn check_table(table: &Table, slot: &Slot, exchange: bool) -> Result<(), String> {
    let found: Vec<&str> = table
        .partitions
        .iter()
        .map(|partition| partition.name.as_deref().unwrap_or(""))
        .collect();
    let wanted = labels(slot, exchange);
    if found != wanted {
        return Err(format!(
            "sfdisk wrote the partitions {}, not {}.",
            found.join(", "),
            wanted.join(", ")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use librift::disk::{Partition, read_table};

    use super::*;

    #[test]
    fn exchange_sizes_are_binary_units() {
        assert_eq!(exchange_size("8G"), Ok(Some(8 * GIB)));
        assert_eq!(exchange_size("8GiB"), Ok(Some(8 * GIB)));
        assert_eq!(exchange_size("512m"), Ok(Some(512 * MIB)));
        assert_eq!(exchange_size("1G"), Ok(Some(GIB)));
        assert_eq!(exchange_size("0"), Ok(None));
        assert_eq!(exchange_size("0G"), Ok(None));
        assert_eq!(exchange_size("none"), Ok(None));
        for wrong in ["8", "8T", "G", "-1G", "1.5G", "32M", "99999999999999999G"] {
            assert!(exchange_size(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn a_folder_of_models_is_its_files() {
        let folder = std::env::temp_dir().join(format!("rift-flash-models-{}", std::process::id()));
        fs::create_dir_all(folder.join("inner")).unwrap();
        assert!(models_in(&folder).unwrap_err().ends_with("holds no files."));
        fs::write(folder.join("b.gguf"), [1; 10]).unwrap();
        fs::write(folder.join("a.gguf"), [1; 5]).unwrap();
        assert_eq!(
            models_in(&folder),
            Ok((vec![folder.join("a.gguf"), folder.join("b.gguf")], 15))
        );
        fs::remove_dir_all(&folder).unwrap();
        assert!(
            models_in(&folder)
                .unwrap_err()
                .starts_with("Could not read")
        );
    }

    #[test]
    fn only_zeros_are_empty() {
        assert!(is_empty(&[0; 4096]));
        let mut table = [0; 4096];
        table[510] = 0x55;
        assert!(!is_empty(&table));
    }

    /// What sfdisk prints for a file rift-flash wrote a table into, with an exchange partition.
    const WRITTEN: &str = r#"{
       "partitiontable": {
          "label": "gpt", "id": "5D6A2E1C-9B7F-4A3D-8E2B-1C0D9E8F7A6B", "device": "drive.img",
          "unit": "sectors", "firstlba": 2048, "lastlba": 50331614, "sectorsize": 512,
          "partitions": [
             {"node": "drive.img1", "start": 2048, "size": 2097152, "type": "C12A7328-F81F-11D2-BA4B-00A0C93EC93B",
              "uuid": "0B1C2D3E-4F50-4162-8374-859607A8B9CA", "name": "esp"},
             {"node": "drive.img2", "start": 2099200, "size": 2097152, "type": "77FF5F63-E7B6-4633-ACF4-1565B864C0E6",
              "uuid": "D47A8035-B6EE-3CD9-799B-BB886EFAEAFB", "name": "store-verity_0.1.0", "attrs": "GUID:60"},
             {"node": "drive.img3", "start": 4196352, "size": 16777216, "type": "8484680C-9521-48C6-9C11-B0720656F69E",
              "uuid": "229D80B9-7A5A-031B-79F2-324122B3A4F0", "name": "store_0.1.0", "attrs": "GUID:60"},
             {"node": "drive.img4", "start": 20973568, "size": 2097152, "type": "77FF5F63-E7B6-4633-ACF4-1565B864C0E6",
              "uuid": "1C2D3E4F-5061-4273-8485-9607A8B9CADB", "name": "_empty"},
             {"node": "drive.img5", "start": 23070720, "size": 16777216, "type": "8484680C-9521-48C6-9C11-B0720656F69E",
              "uuid": "2D3E4F50-6172-4384-9596-07A8B9CADBEC", "name": "_empty"},
             {"node": "drive.img6", "start": 39847936, "size": 2097152, "type": "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7",
              "uuid": "3E4F5061-7283-4495-A607-A8B9CADBECFD", "name": "exchange"},
             {"node": "drive.img7", "start": 41945088, "size": 8386527, "type": "0FC63DAF-8483-4772-8E79-3D69D8477DE4",
              "uuid": "4F506172-8394-45A6-B7A8-B9CADBECFD0E", "name": "persist"}
          ]
       }
    }"#;

    fn slot(table: &Table) -> Slot {
        Slot {
            version: "0.1.0".into(),
            verity: table.partitions[1].clone(),
            store: table.partitions[2].clone(),
        }
    }

    #[test]
    fn the_written_table_has_to_be_the_one_asked_for() {
        let table = read_table(WRITTEN).unwrap();
        let slot = slot(&table);
        assert_eq!(check_table(&table, &slot, true), Ok(()));
        assert!(
            check_table(&table, &slot, false)
                .unwrap_err()
                .contains("not esp, store-verity_0.1.0, store_0.1.0, _empty, _empty, persist")
        );
        let mut short = table.clone();
        short.partitions.truncate(6);
        assert!(check_table(&short, &slot, true).is_err());
        let mut unnamed = table;
        unnamed.partitions[0] = Partition {
            name: None,
            ..unnamed.partitions[0].clone()
        };
        assert!(check_table(&unnamed, &slot, true).is_err());
    }
}
