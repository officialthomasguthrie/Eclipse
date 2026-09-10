//! The outputs this machine has, read from the DRM connectors in sysfs.
//!
//! Every connected connector becomes a [`Display`]. What the connector says about itself comes
//! from its EDID, and plenty of machines give none: a virtual machine, a KVM switch, a cheap
//! adapter. That is not an error, the size and the mode are simply unknown and the scale falls
//! back to 1.

use std::fs;
use std::path::Path;

use crate::edid;

/// Above this many dots per inch a screen is dense enough to want everything drawn twice as big.
pub const HIDPI: f64 = 160.0;

/// Centimetres in an inch.
const CM_PER_INCH: f64 = 2.54;

/// One connected output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    /// The connector, as the kernel names it: `eDP-1`, `DP-2`, `Virtual-1`.
    pub connector: String,
    /// Preferred mode in pixels, `(0, 0)` when the output does not say.
    pub mode: (u32, u32),
    /// Physical size in centimetres, `(0, 0)` when the output does not say.
    pub size_cm: (u32, u32),
    /// 1 or 2. An output that does not say enough gets 1.
    pub scale: u32,
}

impl Default for Display {
    /// An output nothing is known about yet: no name, no mode, no size, scale 1.
    fn default() -> Self {
        Self {
            connector: String::new(),
            mode: (0, 0),
            size_cm: (0, 0),
            scale: 1,
        }
    }
}

impl Display {
    /// Reads one connector directory, `None` when it is not a connected output.
    fn read(dir: &Path, connector: &str) -> Option<Self> {
        let status = fs::read_to_string(dir.join("status")).ok()?;
        if status.trim() != "connected" {
            return None;
        }
        let edid = fs::read(dir.join("edid"))
            .ok()
            .and_then(|b| edid::parse(&b));
        let mode = edid.and_then(|e| e.mode).unwrap_or((0, 0));
        let size_cm = edid.and_then(|e| e.size_cm).unwrap_or((0, 0));
        Some(Self {
            connector: connector.to_owned(),
            mode,
            size_cm,
            scale: scale_for(mode, size_cm),
        })
    }
}

/// Reads the connectors of the running machine.
#[must_use]
pub fn detect() -> Vec<Display> {
    read(Path::new("/sys/class/drm"))
}

/// Reads the connectors under `drm_dir`, sorted by connector name. A missing directory reads as
/// no outputs at all, so this works on a machine with no graphics and in tests elsewhere.
#[must_use]
pub fn read(drm_dir: &Path) -> Vec<Display> {
    let mut out: Vec<Display> = fs::read_dir(drm_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let connector = connector_of(&name)?;
            Display::read(&entry.path(), connector)
        })
        .collect();
    out.sort_by(|a, b| a.connector.cmp(&b.connector));
    out
}

/// `card0-eDP-1` names the connector `eDP-1`. Anything else, `card0` or `renderD128`, is not a
/// connector directory.
fn connector_of(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("card")?;
    let (number, connector) = rest.split_once('-')?;
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) || connector.is_empty() {
        return None;
    }
    Some(connector)
}

/// The scale heuristic from the master plan: work the dots per inch out of the diagonal, and
/// anything denser than [`HIDPI`] gets 2. Without both a mode and a size there is nothing to
/// work with, so the answer is 1.
#[must_use]
pub fn scale_for(mode: (u32, u32), size_cm: (u32, u32)) -> u32 {
    match dpi(mode, size_cm) {
        Some(dpi) if dpi > HIDPI => 2,
        _ => 1,
    }
}

/// Dots per inch along the diagonal, `None` when the output does not report enough.
#[must_use]
pub fn dpi(mode: (u32, u32), size_cm: (u32, u32)) -> Option<f64> {
    if mode.0 == 0 || mode.1 == 0 || size_cm.0 == 0 || size_cm.1 == 0 {
        return None;
    }
    let pixels = f64::from(mode.0).hypot(f64::from(mode.1));
    let inches = f64::from(size_cm.0).hypot(f64::from(size_cm.1)) / CM_PER_INCH;
    Some(pixels / inches)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same base block the edid tests build, kept here so this module tests on its own.
    fn edid_block(width_cm: u8, height_cm: u8, mode: (u32, u32)) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[..8].copy_from_slice(&[0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]);
        b[21] = width_cm;
        b[22] = height_cm;
        b[54] = 0x01;
        b[56] = u8::try_from(mode.0 & 0xff).unwrap();
        b[58] = u8::try_from((mode.0 >> 8) << 4).unwrap();
        b[59] = u8::try_from(mode.1 & 0xff).unwrap();
        b[61] = u8::try_from((mode.1 >> 8) << 4).unwrap();
        let sum = b.iter().fold(0u8, |acc, x| acc.wrapping_add(*x));
        b[127] = 0u8.wrapping_sub(sum);
        b
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("syzygy-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn connector(drm: &Path, name: &str, status: &str, edid: Option<Vec<u8>>) {
        let dir = drm.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("status"), format!("{status}\n")).unwrap();
        fs::write(dir.join("edid"), edid.unwrap_or_default()).unwrap();
    }

    #[test]
    fn a_laptop_with_a_monitor_plugged_in() {
        let drm = temp_dir("drm-two");
        connector(
            &drm,
            "card0-eDP-1",
            "connected",
            Some(edid_block(30, 19, (2880, 1800))),
        );
        connector(
            &drm,
            "card0-DP-1",
            "connected",
            Some(edid_block(60, 34, (2560, 1440))),
        );
        connector(&drm, "card0-HDMI-A-1", "disconnected", None);
        fs::create_dir_all(drm.join("card0")).unwrap();
        fs::create_dir_all(drm.join("renderD128")).unwrap();

        let found = read(&drm);
        fs::remove_dir_all(&drm).unwrap();

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].connector, "DP-1");
        assert_eq!(found[0].mode, (2560, 1440));
        assert_eq!(found[0].scale, 1);
        assert_eq!(found[1].connector, "eDP-1");
        assert_eq!(found[1].size_cm, (30, 19));
        assert_eq!(found[1].scale, 2);
    }

    #[test]
    fn a_virtual_output_with_no_edid_is_still_an_output() {
        let drm = temp_dir("drm-virtual");
        connector(&drm, "card0-Virtual-1", "connected", None);

        let found = read(&drm);
        fs::remove_dir_all(&drm).unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].connector, "Virtual-1");
        assert_eq!(found[0].mode, (0, 0));
        assert_eq!(found[0].size_cm, (0, 0));
        assert_eq!(found[0].scale, 1);
    }

    #[test]
    fn a_machine_with_no_drm_at_all() {
        assert!(read(Path::new("/nonexistent/drm")).is_empty());
    }

    #[test]
    fn connector_names() {
        assert_eq!(connector_of("card0-eDP-1"), Some("eDP-1"));
        assert_eq!(connector_of("card1-HDMI-A-2"), Some("HDMI-A-2"));
        assert_eq!(connector_of("card0"), None);
        assert_eq!(connector_of("renderD128"), None);
        assert_eq!(connector_of("card0-"), None);
        assert_eq!(connector_of("cardX-DP-1"), None);
    }

    #[test]
    fn the_scale_heuristic() {
        // a 14 inch 2880x1800 panel is about 243 dpi
        assert_eq!(scale_for((2880, 1800), (30, 19)), 2);
        // a 27 inch 2560x1440 monitor is about 109 dpi
        assert_eq!(scale_for((2560, 1440), (60, 34)), 1);
        // a 27 inch 4k monitor is about 163 dpi, just over the line
        assert_eq!(scale_for((3840, 2160), (60, 34)), 2);
        // a 24 inch 1080p monitor is about 92 dpi
        assert_eq!(scale_for((1920, 1080), (53, 30)), 1);
        // nothing to go on
        assert_eq!(scale_for((0, 0), (0, 0)), 1);
        assert_eq!(scale_for((1920, 1080), (0, 0)), 1);
        assert_eq!(dpi((0, 0), (30, 19)), None);
        assert!((dpi((2880, 1800), (30, 19)).unwrap() - 243.0).abs() < 1.0);
    }
}
