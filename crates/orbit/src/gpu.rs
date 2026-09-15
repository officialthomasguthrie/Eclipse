//! Which graphics stack this machine wants.
//!
//! Decision record 0012: Mesa and NVK by default, and the proprietary NVIDIA userspace only on
//! an enrolled host that downloaded it. So all this does is name the vendor of the primary card
//! and the path that goes with it. Nothing here installs anything.

use std::fs;
use std::path::Path;

use crate::host::hex_id;

/// PCI vendor ids we know what to do with.
const KNOWN: [(&str, &str, &str); 3] = [
    ("8086", "intel", MESA),
    ("1002", "amd", MESA),
    ("10de", "nvidia", NVK),
];

/// The open Mesa drivers, which is what Intel and AMD want.
pub const MESA: &str = "mesa";
/// Mesa's open Vulkan driver for NVIDIA cards.
pub const NVK: &str = "nvk";
/// No accelerated path: software rendering, which is what a virtual machine gets.
pub const NONE: &str = "none";
/// A card whose vendor we do not recognise, or no card at all.
pub const UNKNOWN_VENDOR: &str = "unknown";

/// Reads the cards of the running machine.
#[must_use]
pub fn detect() -> (String, String) {
    read(Path::new("/sys/class/drm"))
}

/// The vendor and the path for the primary card under `drm_dir`, as `(vendor, path)`.
///
/// The primary card is the lowest numbered one, which is the card the firmware drew on. A vendor
/// we do not have a path for, and a machine with no card, both come out as unknown and no path.
#[must_use]
pub fn read(drm_dir: &Path) -> (String, String) {
    let mut cards: Vec<(u32, String)> = fs::read_dir(drm_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let number = card_number(&name)?;
            let vendor = fs::read_to_string(entry.path().join("device/vendor")).ok()?;
            Some((number, hex_id(vendor.trim())?))
        })
        .collect();
    cards.sort_unstable();
    let Some((_, vendor)) = cards.first() else {
        return (UNKNOWN_VENDOR.to_owned(), NONE.to_owned());
    };
    KNOWN.iter().find(|(id, _, _)| id == vendor).map_or_else(
        || (UNKNOWN_VENDOR.to_owned(), NONE.to_owned()),
        |(_, name, path)| ((*name).to_owned(), (*path).to_owned()),
    )
}

/// `card0` is card 0. `card0-eDP-1` is a connector, not a card.
fn card_number(name: &str) -> Option<u32> {
    name.strip_prefix("card")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("orbit-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn card(drm: &Path, name: &str, vendor: Option<&str>) {
        let dir = drm.join(name).join("device");
        fs::create_dir_all(&dir).unwrap();
        if let Some(vendor) = vendor {
            fs::write(dir.join("vendor"), format!("{vendor}\n")).unwrap();
        }
    }

    fn read_in(name: &str, build: impl Fn(&Path)) -> (String, String) {
        let drm = temp_dir(name);
        build(&drm);
        let found = read(&drm);
        fs::remove_dir_all(&drm).unwrap();
        found
    }

    #[test]
    fn an_intel_laptop() {
        let found = read_in("gpu-intel", |drm| {
            card(drm, "card0", Some("0x8086"));
            fs::create_dir_all(drm.join("card0-eDP-1")).unwrap();
        });
        assert_eq!(found, ("intel".to_owned(), MESA.to_owned()));
    }

    #[test]
    fn the_primary_card_wins() {
        let found = read_in("gpu-hybrid", |drm| {
            card(drm, "card1", Some("0x10de"));
            card(drm, "card0", Some("0x8086"));
        });
        assert_eq!(found, ("intel".to_owned(), MESA.to_owned()));
    }

    #[test]
    fn a_desktop_with_one_nvidia_card() {
        let found = read_in("gpu-nvidia", |drm| card(drm, "card0", Some("0x10DE")));
        assert_eq!(found, ("nvidia".to_owned(), NVK.to_owned()));
    }

    #[test]
    fn an_amd_desktop() {
        let found = read_in("gpu-amd", |drm| card(drm, "card0", Some("0x1002")));
        assert_eq!(found, ("amd".to_owned(), MESA.to_owned()));
    }

    #[test]
    fn a_virtual_machine_has_no_path() {
        // virtio hangs the drm device off the virtio bus, so there is no pci vendor to read
        let found = read_in("gpu-virtual", |drm| card(drm, "card0", None));
        assert_eq!(found, (UNKNOWN_VENDOR.to_owned(), NONE.to_owned()));
        // and a card whose vendor we have no path for is the same answer
        let found = read_in("gpu-other", |drm| card(drm, "card0", Some("0x1af4")));
        assert_eq!(found, (UNKNOWN_VENDOR.to_owned(), NONE.to_owned()));
        assert_eq!(
            read(Path::new("/nonexistent/drm")),
            (UNKNOWN_VENDOR.to_owned(), NONE.to_owned())
        );
    }

    #[test]
    fn card_numbers() {
        assert_eq!(card_number("card0"), Some(0));
        assert_eq!(card_number("card12"), Some(12));
        assert_eq!(card_number("card0-eDP-1"), None);
        assert_eq!(card_number("renderD128"), None);
    }
}
