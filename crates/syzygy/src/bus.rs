//! Syzygy on the system bus: `dev.eclipse.Syzygy` at `/dev/eclipse/Syzygy`.
//!
//! Read only for now. The profile is worked out once, before the name is taken, so anything that
//! waits for the name has the answers the moment it arrives. Changing a setting from the bus
//! comes with `eclipse host`.

use libeclipse::Component;

use crate::profile::Profile;

/// One output, as the bus reports it: connector, width, height, scale. A width of 0 means the
/// output gave no EDID, which is what a KVM switch and a cheap adapter look like.
pub type BusDisplay = (String, u32, u32, u32);

/// The object that answers on the bus.
pub struct Syzygy {
    profile: Profile,
}

#[zbus::interface(name = "dev.eclipse.Syzygy")]
impl Syzygy {
    /// SHA-256 of the machine's DMI strings and PCI ids.
    #[zbus(property)]
    fn fingerprint(&self) -> String {
        self.profile.identity.fingerprint.clone()
    }

    /// `owned`, `trusted` or `borrowed`.
    #[zbus(property)]
    fn class(&self) -> String {
        self.profile.settings.class.clone()
    }

    /// Every connected output.
    #[zbus(property)]
    fn displays(&self) -> Vec<BusDisplay> {
        self.profile
            .settings
            .displays
            .iter()
            .map(|d| (d.connector.clone(), d.mode.0, d.mode.1, d.scale))
            .collect()
    }

    /// `mesa`, `nvk` or `none`.
    #[zbus(property)]
    fn gpu_path(&self) -> String {
        self.profile.settings.gpu_path.clone()
    }

    /// Which Aura model tier this machine can carry.
    #[zbus(property)]
    fn ai_tier(&self) -> String {
        self.profile.settings.ai_tier.clone()
    }
}

/// Takes the name and answers until the process is stopped.
///
/// # Errors
///
/// When the system bus is not there, or another process already owns the name.
pub fn serve(profile: Profile) -> zbus::Result<()> {
    let component = Component::Syzygy;
    let _connection = zbus::blocking::connection::Builder::system()?
        .name(component.dbus_name())?
        .serve_at(component.dbus_path(), Syzygy { profile })?
        .build()?;
    // the connection runs on its own threads; this one has nothing left to do
    loop {
        std::thread::park();
    }
}
