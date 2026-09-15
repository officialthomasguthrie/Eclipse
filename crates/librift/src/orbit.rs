//! Orbit from a client's side: what it remembers about this machine, read off the system bus.

#[cfg(feature = "bus")]
use crate::{Component, bus};

/// One connected output, as Orbit reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// The connector, for example `eDP-1`.
    pub connector: String,
    /// Width of the preferred mode in pixels. 0 when the output gave no EDID.
    pub width: u32,
    /// Height of the preferred mode in pixels.
    pub height: u32,
    /// The scale the session draws it at.
    pub scale: u32,
}

/// The settings Orbit works with for this machine: its defaults, then what it detected, then
/// what a person set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    /// SHA-256 of the machine's DMI strings and PCI ids, in hex.
    pub fingerprint: String,
    /// `owned`, `trusted` or `borrowed`.
    pub class: String,
    /// Every connected output.
    pub outputs: Vec<Output>,
    /// `mesa`, `nvk` or `none`.
    pub gpu_path: String,
    /// Which Quasar model tier the machine can carry.
    pub ai_tier: String,
}

/// Reads Orbit's properties.
///
/// # Errors
///
/// A sentence when the bus or Orbit is not there, or a property could not be read.
#[cfg(feature = "bus")]
pub fn host() -> Result<Host, String> {
    let orbit = Component::Orbit;
    let connection = bus::connect(bus::PROPERTY_TIMEOUT)?;
    let proxy = bus::proxy(&connection, orbit)?;
    let text = |name: &str| {
        proxy
            .get_property::<String>(name)
            .map_err(|e| bus::sentence(orbit, e))
    };
    let outputs: Vec<(String, u32, u32, u32)> = proxy
        .get_property("Displays")
        .map_err(|e| bus::sentence(orbit, e))?;
    Ok(Host {
        fingerprint: text("Fingerprint")?,
        class: text("Class")?,
        outputs: outputs
            .into_iter()
            .map(|(connector, width, height, scale)| Output {
                connector,
                width,
                height,
                scale,
            })
            .collect(),
        gpu_path: text("GpuPath")?,
        ai_tier: text("AiTier")?,
    })
}
