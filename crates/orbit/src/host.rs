//! What the kernel tells us about the machine: DMI strings and PCI ids from sysfs.

use std::fs;
use std::path::Path;

use crate::sha256;

/// The DMI fields that go into the fingerprint, in this order.
pub const DMI_FIELDS: [&str; 6] = [
    "sys_vendor",
    "product_name",
    "product_version",
    "board_vendor",
    "board_name",
    "bios_version",
];

/// The stable identity of a machine: DMI strings and the PCI devices it has.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Host {
    /// One value per entry of [`DMI_FIELDS`], empty when the machine does not provide it.
    pub dmi: [String; 6],
    /// `vendor:device` pairs, lower-case hex, sorted, one per device.
    pub pci: Vec<String>,
}

impl Host {
    /// Reads the running machine through sysfs. Missing files read as empty, so this also works
    /// on machines without DMI and in tests on other systems.
    #[must_use]
    pub fn detect() -> Self {
        Self::read(
            Path::new("/sys/class/dmi/id"),
            Path::new("/sys/bus/pci/devices"),
        )
    }

    /// Reads DMI strings from `dmi_dir` and PCI ids from the device directories in `pci_dir`.
    #[must_use]
    pub fn read(dmi_dir: &Path, pci_dir: &Path) -> Self {
        let dmi = DMI_FIELDS.map(|name| read_trimmed(&dmi_dir.join(name)));
        let mut pci: Vec<String> = fs::read_dir(pci_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let dir = entry.path();
                let vendor = hex_id(&read_trimmed(&dir.join("vendor")))?;
                let device = hex_id(&read_trimmed(&dir.join("device")))?;
                Some(format!("{vendor}:{device}"))
            })
            .collect();
        pci.sort_unstable();
        Self { dmi, pci }
    }

    /// The DMI fields as `(name, value)` pairs.
    pub fn dmi_fields(&self) -> impl Iterator<Item = (&'static str, &str)> {
        DMI_FIELDS
            .iter()
            .copied()
            .zip(self.dmi.iter().map(String::as_str))
    }

    /// The machine in words, for whoever opens the profile. Empty DMI reads as unknown.
    #[must_use]
    pub fn label(&self) -> String {
        let words: Vec<&str> = [self.dmi[0].as_str(), self.dmi[1].as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        if words.is_empty() {
            "unknown machine".to_owned()
        } else {
            words.join(" ")
        }
    }

    /// SHA-256 over the DMI fields and the sorted PCI ids, as hex. Two machines of the same model
    /// with the same cards get the same fingerprint; the serial number is left out on purpose.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut input = String::new();
        for (name, value) in self.dmi_fields() {
            input.push_str(name);
            input.push('=');
            input.push_str(value);
            input.push('\n');
        }
        for id in &self.pci {
            input.push_str("pci=");
            input.push_str(id);
            input.push('\n');
        }
        sha256::hex(input.as_bytes())
    }
}

/// A machine with a battery is a laptop.
pub const LAPTOP: &str = "laptop";
/// Anything else is a desktop.
pub const DESKTOP: &str = "desktop";

/// The AI tiers, smallest first. Orbit picks one from memory; a person can override it.
pub const TIERS: [&str; 3] = ["small", "medium", "large"];

/// Kibibytes in a gibibyte.
const GIB: u64 = 1024 * 1024;

/// Laptop or desktop, from the power supplies the running machine has.
#[must_use]
pub fn chassis() -> &'static str {
    chassis_from(Path::new("/sys/class/power_supply"))
}

/// [`LAPTOP`] when one of the power supplies under `dir` is a battery.
#[must_use]
pub fn chassis_from(dir: &Path) -> &'static str {
    let battery = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| read_trimmed(&entry.path().join("type")) == "Battery");
    if battery { LAPTOP } else { DESKTOP }
}

/// The AI tier for the running machine.
#[must_use]
pub fn ai_tier() -> &'static str {
    tier_for(memory_kib(Path::new("/proc/meminfo")))
}

/// The tier for a machine with `kib` kibibytes of memory. The thresholds sit under the round
/// numbers on purpose: a 16 GB machine reports about 15.5 GiB once the firmware has had its share.
#[must_use]
pub fn tier_for(kib: u64) -> &'static str {
    match kib {
        kib if kib >= 15 * GIB => TIERS[2],
        kib if kib >= 7 * GIB => TIERS[1],
        _ => TIERS[0],
    }
}

/// `MemTotal` from a meminfo file, in kibibytes. 0 when it cannot be read.
#[must_use]
pub fn memory_kib(meminfo: &Path) -> u64 {
    fs::read_to_string(meminfo)
        .unwrap_or_default()
        .lines()
        .find_map(|line| {
            let rest = line.strip_prefix("MemTotal:")?;
            rest.split_whitespace().next()?.parse().ok()
        })
        .unwrap_or(0)
}

fn read_trimmed(path: &Path) -> String {
    fs::read_to_string(path)
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// Turns sysfs's `0x8086` into `8086`. Anything that is not hex is dropped.
pub fn hex_id(raw: &str) -> Option<String> {
    let digits = raw.strip_prefix("0x").unwrap_or(raw);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(digits.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Host {
        Host {
            dmi: [
                "QEMU".into(),
                "Standard PC (Q35 + ICH9, 2009)".into(),
                "pc-q35-9.0".into(),
                String::new(),
                String::new(),
                "rel-1.16.3-0-ga6ed6b701f0a-prebuilt.qemu.org".into(),
            ],
            pci: vec!["1234:1111".into(), "8086:29c0".into()],
        }
    }

    #[test]
    fn fingerprint_is_stable() {
        let a = sample().fingerprint();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(a, sample().fingerprint());
        // pinned so a change in the input layout is noticed
        assert_eq!(a, PINNED);
    }

    const PINNED: &str = "454d32c417c489832cf964b90a36e411b380a5d5a9e9384070c0443ddabbbe3e";

    #[test]
    fn every_part_counts() {
        let base = sample();
        let mut other = sample();
        other.pci.push("10de:2484".into());
        assert_ne!(base.fingerprint(), other.fingerprint());
        let mut other = sample();
        other.dmi[3] = "Acme".into();
        assert_ne!(base.fingerprint(), other.fingerprint());
    }

    #[test]
    fn missing_sysfs_reads_as_empty() {
        let host = Host::read(Path::new("/nonexistent/dmi"), Path::new("/nonexistent/pci"));
        assert_eq!(host, Host::default());
        assert_eq!(host.fingerprint().len(), 64);
    }

    #[test]
    fn reads_a_fake_sysfs() {
        let root = std::env::temp_dir().join(format!("orbit-host-{}", std::process::id()));
        let dmi = root.join("dmi");
        let pci = root.join("pci");
        fs::create_dir_all(&dmi).unwrap();
        fs::create_dir_all(pci.join("0000:00:02.0")).unwrap();
        fs::create_dir_all(pci.join("0000:00:01.0")).unwrap();
        fs::write(dmi.join("sys_vendor"), "QEMU\n").unwrap();
        fs::write(dmi.join("product_name"), "  Standard PC \n").unwrap();
        fs::write(pci.join("0000:00:02.0/vendor"), "0x8086\n").unwrap();
        fs::write(pci.join("0000:00:02.0/device"), "0x29C0\n").unwrap();
        fs::write(pci.join("0000:00:01.0/vendor"), "0x1234\n").unwrap();
        fs::write(pci.join("0000:00:01.0/device"), "0x1111\n").unwrap();

        let host = Host::read(&dmi, &pci);
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(host.dmi[0], "QEMU");
        assert_eq!(host.dmi[1], "Standard PC");
        assert_eq!(host.dmi[2], "");
        assert_eq!(host.pci, vec!["1234:1111", "8086:29c0"]);
    }

    #[test]
    fn a_laptop_has_a_battery() {
        let root = std::env::temp_dir().join(format!("orbit-power-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("AC")).unwrap();
        fs::write(root.join("AC/type"), "Mains\n").unwrap();
        assert_eq!(chassis_from(&root), DESKTOP);
        fs::create_dir_all(root.join("BAT0")).unwrap();
        fs::write(root.join("BAT0/type"), "Battery\n").unwrap();
        assert_eq!(chassis_from(&root), LAPTOP);
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(chassis_from(Path::new("/nonexistent/power")), DESKTOP);
    }

    #[test]
    fn tiers_come_from_memory() {
        // the boot test gives the machine 4096 MB and linux keeps some of it
        assert_eq!(tier_for(3_900_000), "small");
        assert_eq!(tier_for(0), "small");
        // 8 GB reports about 7.7 GiB, 16 GB about 15.5
        assert_eq!(tier_for(8_100_000), "medium");
        assert_eq!(tier_for(16_300_000), "large");
        assert_eq!(tier_for(64 * 1024 * 1024), "large");
    }

    #[test]
    fn meminfo_is_parsed() {
        let path = std::env::temp_dir().join(format!("orbit-meminfo-{}", std::process::id()));
        fs::write(
            &path,
            "MemTotal:        3999504 kB\nMemFree:          123 kB\n",
        )
        .unwrap();
        assert_eq!(memory_kib(&path), 3_999_504);
        fs::remove_file(&path).unwrap();
        assert_eq!(memory_kib(Path::new("/nonexistent/meminfo")), 0);
    }

    #[test]
    fn hex_ids_are_normalised() {
        assert_eq!(hex_id("0x8086").as_deref(), Some("8086"));
        assert_eq!(hex_id("29C0").as_deref(), Some("29c0"));
        assert_eq!(hex_id(""), None);
        assert_eq!(hex_id("0xzz"), None);
    }
}
