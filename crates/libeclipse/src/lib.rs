//! Names, D-Bus addresses and paths shared by every Eclipse component.
//!
//! Apache-2.0 so other people can embed it. Keep it dependency free.

/// Version of the Eclipse workspace this crate was built from.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Namespace every Eclipse D-Bus service lives under.
pub const DBUS_PREFIX: &str = "dev.eclipse";

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

    /// Well-known D-Bus bus name, for example `dev.eclipse.Aura`.
    #[must_use]
    pub fn dbus_name(self) -> String {
        format!("{DBUS_PREFIX}.{}", self.display_name())
    }

    /// D-Bus object path, for example `/dev/eclipse/Aura`.
    #[must_use]
    pub fn dbus_path(self) -> String {
        format!("/{}/{}", DBUS_PREFIX.replace('.', "/"), self.display_name())
    }
}

/// Where things live on a running Eclipse system. All of these are on the persist partition,
/// except what the image itself installs under /etc.
pub mod paths {
    /// The model manifest, installed by the image.
    pub const MODEL_MANIFEST: &str = "/etc/eclipse/models.toml";
    /// Top level of the persist volume.
    pub const PERSIST: &str = "/persist";
    /// GGUF weights and voices (`@models`).
    pub const MODELS: &str = "/var/lib/eclipse/models";
    /// Per-host profiles written by Syzygy (`@hosts`).
    pub const HOSTS: &str = "/var/lib/eclipse/hosts";
    /// Aura's index and action log.
    pub const AURA_STATE: &str = "/var/lib/eclipse/aura";
    /// Phase marker written by the image.
    pub const PHASE: &str = "/etc/eclipse/phase";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbus_names_follow_the_prefix() {
        assert_eq!(Component::Aura.dbus_name(), "dev.eclipse.Aura");
        assert_eq!(Component::Aura.dbus_path(), "/dev/eclipse/Aura");
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = Component::ALL.iter().map(|c| c.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Component::ALL.len());
    }
}
