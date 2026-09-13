//! Partition tables: what sfdisk prints about one, and the table Eclipse writes onto a new drive.

use serde::Deserialize;

use super::{
    BASIC_DATA_TYPE, ESP_SIZE, ESP_TYPE, LINUX_TYPE, MIB, SECTOR, STORE_SIZE, USR_TYPE,
    USR_VERITY_TYPE, VERITY_SIZE,
};

/// A partition in what `sfdisk --json` printed, or in a table read from an image.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Partition {
    /// The partition's device as sfdisk names it, or its number in a table read from an image.
    pub node: String,
    /// Where it starts, in sectors.
    pub start: u64,
    /// In sectors.
    pub size: u64,
    /// Its GPT type, in upper case.
    #[serde(rename = "type")]
    pub kind: String,
    /// Its partition uuid, in upper case.
    pub uuid: Option<String>,
    /// Its label.
    pub name: Option<String>,
    /// Its attribute bits, written the way sfdisk writes them: `GUID:60`.
    pub attrs: Option<String>,
}

/// A partition table.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Table {
    /// The size of a sector in bytes.
    #[serde(default = "sector")]
    pub sectorsize: u64,
    /// The partitions in the order of the table.
    pub partitions: Vec<Partition>,
}

fn sector() -> u64 {
    SECTOR
}

impl Table {
    /// The size of `partition` in bytes.
    #[must_use]
    pub fn bytes(&self, partition: &Partition) -> u64 {
        partition.size * self.sectorsize
    }

    /// Where `partition` starts, in bytes.
    #[must_use]
    pub fn offset(&self, partition: &Partition) -> u64 {
        partition.start * self.sectorsize
    }

    /// The first partition labelled `name`.
    #[must_use]
    pub fn named(&self, name: &str) -> Option<&Partition> {
        self.partitions
            .iter()
            .find(|partition| partition.name.as_deref() == Some(name))
    }
}

/// The partition table in what `sfdisk --json` printed.
///
/// # Errors
///
/// When sfdisk printed something that is not a partition table.
pub fn read_table(json: &str) -> Result<Table, String> {
    #[derive(Deserialize)]
    struct Sfdisk {
        partitiontable: Table,
    }
    serde_json::from_str::<Sfdisk>(json)
        .map(|found| found.partitiontable)
        .map_err(|e| format!("sfdisk printed a partition table Eclipse cannot read: {e}"))
}

/// A slot that holds a version: its verity partition and its store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// The version in the partitions' labels.
    pub version: String,
    /// The verity partition, `store-verity_<version>`.
    pub verity: Partition,
    /// The store, `store_<version>`.
    pub store: Partition,
}

/// The sfdisk script for a new drive: the esp, `slot` as slot A with the uuids its uki looks for, an
/// empty slot B, the exchange partition when it has `exchange` bytes, and persist in the rest.
#[must_use]
pub fn script(slot: &Slot, exchange: Option<u64>) -> String {
    let mut lines = vec![
        "label: gpt".to_string(),
        format!("size={}MiB, type={ESP_TYPE}, name=\"esp\"", ESP_SIZE / MIB),
    ];
    for (partition, bytes) in [(&slot.verity, VERITY_SIZE), (&slot.store, STORE_SIZE)] {
        let mut fields = vec![
            format!("size={}MiB", bytes / MIB),
            format!("type={}", partition.kind),
        ];
        if let Some(uuid) = &partition.uuid {
            fields.push(format!("uuid={uuid}"));
        }
        if let Some(name) = &partition.name {
            fields.push(format!("name=\"{name}\""));
        }
        if let Some(attrs) = partition.attrs.as_deref().filter(|attrs| !attrs.is_empty()) {
            fields.push(format!("attrs=\"{attrs}\""));
        }
        lines.push(fields.join(", "));
    }
    lines.push(format!(
        "size={}MiB, type={USR_VERITY_TYPE}, name=\"_empty\"",
        VERITY_SIZE / MIB
    ));
    lines.push(format!(
        "size={}MiB, type={USR_TYPE}, name=\"_empty\"",
        STORE_SIZE / MIB
    ));
    if let Some(bytes) = exchange {
        lines.push(format!(
            "size={}MiB, type={BASIC_DATA_TYPE}, name=\"exchange\"",
            bytes.div_ceil(MIB)
        ));
    }
    lines.push(format!("type={LINUX_TYPE}, name=\"persist\""));
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::super::GIB;
    use super::*;

    /// A running drive after an update and a rollback: 0.2.0 runs from slot b, 0.3.0 is in slot a.
    pub const TABLE: &str = r#"{
       "partitiontable": {
          "label": "gpt", "id": "B581FEF7-24ED-4F31-990B-099EC86BBA03", "device": "/dev/nvme0n1",
          "unit": "sectors", "firstlba": 2048, "lastlba": 44171264, "sectorsize": 512,
          "partitions": [
             {"node": "/dev/nvme0n1p1", "start": 2048, "size": 2097152, "type": "C12A7328-F81F-11D2-BA4B-00A0C93EC93B",
              "uuid": "3A5A4E2B-1B1C-4F53-9D2E-0E4F1B2C3D4E", "name": "esp"},
             {"node": "/dev/nvme0n1p2", "start": 2099200, "size": 2097152, "type": "77FF5F63-E7B6-4633-ACF4-1565B864C0E6",
              "uuid": "06072C77-78C7-FA9E-7540-E489822F25FD", "name": "store-verity_0.3.0", "attrs": "GUID:60"},
             {"node": "/dev/nvme0n1p3", "start": 4196352, "size": 16777216, "type": "8484680C-9521-48C6-9C11-B0720656F69E",
              "uuid": "18285BF3-EA77-2240-3976-E8CD9FC1FBB6", "name": "store_0.3.0", "attrs": "GUID:60"},
             {"node": "/dev/nvme0n1p4", "start": 20973568, "size": 2097152, "type": "77FF5F63-E7B6-4633-ACF4-1565B864C0E6",
              "uuid": "5C1D8E0A-2B3C-4D5E-8F90-A1B2C3D4E5F6", "name": "store-verity_0.2.0", "attrs": "GUID:60"},
             {"node": "/dev/nvme0n1p5", "start": 23070720, "size": 16777216, "type": "8484680C-9521-48C6-9C11-B0720656F69E",
              "uuid": "7E8F9A0B-1C2D-4E3F-9051-627384950A1B", "name": "store_0.2.0"},
             {"node": "/dev/nvme0n1p6", "start": 39847936, "size": 4323328, "type": "0FC63DAF-8483-4772-8E79-3D69D8477DE4",
              "uuid": "9A0B1C2D-3E4F-4051-8263-748596A7B8C9", "name": "persist"}
          ]
       }
    }"#;

    #[test]
    fn sfdisk_tables_are_read_with_sizes_and_offsets() {
        let table = read_table(TABLE).unwrap();
        let esp = table.named("esp").unwrap();
        assert_eq!(table.bytes(esp), GIB);
        assert_eq!(table.offset(esp), MIB);
        assert_eq!(table.named("exchange"), None);
        assert_eq!(
            table.partitions[1].attrs.as_deref(),
            Some("GUID:60"),
            "{table:?}"
        );
        assert!(read_table("[]").is_err());
    }

    #[test]
    fn a_new_drive_gets_the_slot_as_slot_a_and_an_empty_slot_b() {
        let table = read_table(TABLE).unwrap();
        let slot = Slot {
            version: "0.2.0".into(),
            verity: table.partitions[3].clone(),
            store: table.partitions[4].clone(),
        };
        assert_eq!(
            script(&slot, None),
            "label: gpt\n\
             size=1024MiB, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=\"esp\"\n\
             size=1024MiB, type=77FF5F63-E7B6-4633-ACF4-1565B864C0E6, uuid=5C1D8E0A-2B3C-4D5E-8F90-A1B2C3D4E5F6, name=\"store-verity_0.2.0\", attrs=\"GUID:60\"\n\
             size=8192MiB, type=8484680C-9521-48C6-9C11-B0720656F69E, uuid=7E8F9A0B-1C2D-4E3F-9051-627384950A1B, name=\"store_0.2.0\"\n\
             size=1024MiB, type=77FF5F63-E7B6-4633-ACF4-1565B864C0E6, name=\"_empty\"\n\
             size=8192MiB, type=8484680C-9521-48C6-9C11-B0720656F69E, name=\"_empty\"\n\
             type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name=\"persist\"\n"
        );
        let with_exchange = script(&slot, Some(8 * GIB));
        let lines: Vec<&str> = with_exchange.lines().collect();
        assert_eq!(lines.len(), 8);
        assert_eq!(
            lines[6],
            "size=8192MiB, type=EBD0A0A2-B9E5-4433-87C0-68B6B72699C7, name=\"exchange\""
        );
        assert!(lines[7].ends_with("name=\"persist\""));
        // an exchange partition that is not whole MiB rounds up
        assert!(
            script(&slot, Some(GIB + 1))
                .lines()
                .any(|line| line.starts_with("size=1025MiB, type=EBD0A0A2"))
        );
    }
}
