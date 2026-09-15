//! Names, D-Bus addresses and paths shared by every Rift component, the OS commands Corona and
//! the rift command run, the client side of the services on the system bus, the index for
//! search by meaning, what the system calls itself, and how a drive is written.
//!
//! Apache-2.0 so other people can embed it. Keep it dependency free: only the `bus` feature,
//! which the asking side of the bus turns on, brings in zbus, and only the `disk` feature serde and
//! getrandom.

pub mod aura;
pub mod bus;
#[cfg(feature = "disk")]
pub mod disk;
pub mod os;
pub mod penumbra;
pub mod release;
pub mod search;
pub mod syzygy;
pub mod vault;

/// Version of the Rift workspace this crate was built from.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Namespace every Rift D-Bus service lives under.
pub const DBUS_PREFIX: &str = "dev.rift";

/// The first-party components, in the order they come up during boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Component {
    /// The immutable system image and the boot sequence.
    Totality,
    /// Host detection, adaptation, and per-machine memory.
    Syzygy,
    /// The Wayland compositor.
    Umbra,
    /// The omnibar shell.
    Corona,
    /// The local AI service.
    Aura,
    /// App sandboxing and permissions.
    Penumbra,
    /// Snapshots, backups, and drive cloning.
    Vault,
}

impl Component {
    /// Every component, in boot order.
    pub const ALL: [Component; 7] = [
        Component::Totality,
        Component::Syzygy,
        Component::Umbra,
        Component::Corona,
        Component::Aura,
        Component::Penumbra,
        Component::Vault,
    ];

    /// Lower-case name, as used in paths, unit names, and logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Component::Totality => "totality",
            Component::Syzygy => "syzygy",
            Component::Umbra => "umbra",
            Component::Corona => "corona",
            Component::Aura => "aura",
            Component::Penumbra => "penumbra",
            Component::Vault => "vault",
        }
    }

    /// Capitalised name, as shown to people.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Component::Totality => "Totality",
            Component::Syzygy => "Syzygy",
            Component::Umbra => "Umbra",
            Component::Corona => "Corona",
            Component::Aura => "Aura",
            Component::Penumbra => "Penumbra",
            Component::Vault => "Vault",
        }
    }

    /// Well-known D-Bus bus name, for example `dev.rift.Aura`.
    #[must_use]
    pub fn dbus_name(self) -> String {
        format!("{DBUS_PREFIX}.{}", self.display_name())
    }

    /// D-Bus object path, for example `/dev/rift/Aura`.
    #[must_use]
    pub fn dbus_path(self) -> String {
        format!("/{}/{}", DBUS_PREFIX.replace('.', "/"), self.display_name())
    }
}

/// Where things live on a running Rift system. All of these are on the persist partition,
/// except what the image itself installs under /etc.
pub mod paths {
    /// The model manifest, installed by the image.
    pub const MODEL_MANIFEST: &str = "/etc/rift/models.toml";
    /// Top level of the persist volume.
    pub const PERSIST: &str = "/persist";
    /// GGUF weights and voices (`@models`).
    pub const MODELS: &str = "/var/lib/rift/models";
    /// Per-host profiles written by Syzygy (`@hosts`).
    pub const HOSTS: &str = "/var/lib/rift/hosts";
    /// Aura's index and action log.
    pub const AURA_STATE: &str = "/var/lib/rift/aura";
    /// Which apps have their network off, kept by Penumbra.
    pub const PENUMBRA_STATE: &str = "/var/lib/rift/penumbra";
    /// Phase marker written by the image.
    pub const PHASE: &str = "/etc/rift/phase";
    /// The logo in characters, without its colours, installed by the image.
    pub const LOGO: &str = "/etc/rift/logo.txt";
    /// The same logo with its colours as terminal escape sequences.
    pub const LOGO_ANSI: &str = "/etc/rift/logo.ansi";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbus_names_follow_the_prefix() {
        assert_eq!(Component::Aura.dbus_name(), "dev.rift.Aura");
        assert_eq!(Component::Aura.dbus_path(), "/dev/rift/Aura");
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = Component::ALL.iter().map(|c| c.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Component::ALL.len());
    }
}
