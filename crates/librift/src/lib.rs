//! Names, D-Bus addresses and paths shared by every Rift component, the OS commands Lens and
//! the rift command run, the client side of the services on the system bus, the index for
//! search by meaning, what the system calls itself, and how a drive is written.
//!
//! Apache-2.0 so other people can embed it. Keep it dependency free: only the `bus` feature,
//! which the asking side of the bus turns on, brings in zbus, and only the `disk` feature serde and
//! getrandom.

pub mod airlock;
pub mod bus;
#[cfg(feature = "disk")]
pub mod disk;
pub mod orbit;
pub mod os;
pub mod quasar;
pub mod release;
pub mod search;
pub mod vault;

/// Version of the Rift workspace this crate was built from.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Namespace every Rift D-Bus service lives under.
pub const DBUS_PREFIX: &str = "dev.rift";

/// The first-party components, in the order they come up during boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Component {
    /// The immutable system image and the boot sequence.
    Liftoff,
    /// Host detection, adaptation, and per-machine memory.
    Orbit,
    /// The Wayland compositor.
    Horizon,
    /// The omnibar shell.
    Lens,
    /// The local AI service.
    Quasar,
    /// App sandboxing and permissions.
    Airlock,
    /// Snapshots, backups, and drive cloning.
    Vault,
}

impl Component {
    /// Every component, in boot order.
    pub const ALL: [Component; 7] = [
        Component::Liftoff,
        Component::Orbit,
        Component::Horizon,
        Component::Lens,
        Component::Quasar,
        Component::Airlock,
        Component::Vault,
    ];

    /// Lower-case name, as used in paths, unit names, and logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Component::Liftoff => "liftoff",
            Component::Orbit => "orbit",
            Component::Horizon => "horizon",
            Component::Lens => "lens",
            Component::Quasar => "quasar",
            Component::Airlock => "airlock",
            Component::Vault => "vault",
        }
    }

    /// Capitalised name, as shown to people.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Component::Liftoff => "Liftoff",
            Component::Orbit => "Orbit",
            Component::Horizon => "Horizon",
            Component::Lens => "Lens",
            Component::Quasar => "Quasar",
            Component::Airlock => "Airlock",
            Component::Vault => "Vault",
        }
    }

    /// Well-known D-Bus bus name, for example `dev.rift.Quasar`.
    #[must_use]
    pub fn dbus_name(self) -> String {
        format!("{DBUS_PREFIX}.{}", self.display_name())
    }

    /// D-Bus object path, for example `/dev/rift/Quasar`.
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
    /// Per-host profiles written by Orbit (`@hosts`).
    pub const HOSTS: &str = "/var/lib/rift/hosts";
    /// Quasar's index and action log.
    pub const QUASAR_STATE: &str = "/var/lib/rift/quasar";
    /// Which apps have their network off, kept by Airlock.
    pub const AIRLOCK_STATE: &str = "/var/lib/rift/airlock";
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
        assert_eq!(Component::Quasar.dbus_name(), "dev.rift.Quasar");
        assert_eq!(Component::Quasar.dbus_path(), "/dev/rift/Quasar");
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = Component::ALL.iter().map(|c| c.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Component::ALL.len());
    }
}
