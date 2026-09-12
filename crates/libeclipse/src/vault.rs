//! Vault from a client's side: the snapshots Timeline keeps of home, taking one, and restoring a
//! file from one.

#[cfg(feature = "bus")]
use std::time::Duration;

#[cfg(feature = "bus")]
use crate::{Component, bus};

/// How long taking a snapshot may take. The hourly one can hold the lock for a moment.
#[cfg(feature = "bus")]
const TAKE_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a restore may take. A big file is copied in full.
#[cfg(feature = "bus")]
const RESTORE_TIMEOUT: Duration = Duration::from_secs(1800);

/// The error Vault refuses a restore with when the file at the path has changed.
pub const CHANGED: &str = "org.freedesktop.DBus.Error.FileExists";

/// What a restore did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restored {
    /// Nothing was at the path, the copy is there now.
    Restored,
    /// A different file was at the path and the copy took its place.
    Replaced,
    /// The file at the path already had the same bytes.
    Unchanged,
}

/// Why a restore did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The file at the path has changed since the snapshot. Holds Vault's sentence. Asking again
    /// with `replace` overwrites it.
    Changed(String),
    /// Anything else, as a sentence.
    Other(String),
}

/// What the outcome string `Restore` returned means.
#[must_use]
pub fn read(outcome: &str) -> Option<Restored> {
    match outcome {
        "restored" => Some(Restored::Restored),
        "replaced" => Some(Restored::Replaced),
        "unchanged" => Some(Restored::Unchanged),
        _ => None,
    }
}

/// The snapshots of home, oldest first.
///
/// # Errors
///
/// A sentence when the bus or Vault is not there, or Vault could not read them.
#[cfg(feature = "bus")]
pub fn list() -> Result<Vec<String>, String> {
    let vault = Component::Vault;
    let connection = bus::connect(bus::PROPERTY_TIMEOUT)?;
    let proxy = bus::proxy(&connection, vault)?;
    proxy.call("List", &()).map_err(|e| bus::sentence(vault, e))
}

/// Takes a snapshot of home now and returns its name.
///
/// # Errors
///
/// A sentence when the bus or Vault is not there, or the snapshot could not be taken.
#[cfg(feature = "bus")]
pub fn take() -> Result<String, String> {
    let vault = Component::Vault;
    let connection = bus::connect(TAKE_TIMEOUT)?;
    let proxy = bus::proxy(&connection, vault)?;
    proxy.call("Take", &()).map_err(|e| bus::sentence(vault, e))
}

/// Restores `path`, a full path to a file in home, from `snapshot`. Without `replace` a file that
/// changed since the snapshot is left as it is. Returns what happened and the path written.
///
/// # Errors
///
/// [`Refusal::Changed`] when the file changed and `replace` is false, otherwise a sentence.
#[cfg(feature = "bus")]
pub fn restore(snapshot: &str, path: &str, replace: bool) -> Result<(Restored, String), Refusal> {
    let vault = Component::Vault;
    let connection = bus::connect(RESTORE_TIMEOUT).map_err(Refusal::Other)?;
    let proxy = bus::proxy(&connection, vault).map_err(Refusal::Other)?;
    let (outcome, written): (String, String) = proxy
        .call("Restore", &(snapshot, path, replace))
        .map_err(|e| match changed(&e) {
            Some(why) => Refusal::Changed(why),
            None => Refusal::Other(bus::sentence(vault, e)),
        })?;
    let restored = read(&outcome).ok_or_else(|| {
        Refusal::Other(format!(
            "Vault said \"{outcome}\", which this program does not understand."
        ))
    })?;
    Ok((restored, written))
}

/// Vault's sentence, when the error is the one for a file that changed.
#[cfg(feature = "bus")]
fn changed(error: &zbus::Error) -> Option<String> {
    match error {
        zbus::Error::MethodError(name, detail, _) if name.as_str() == CHANGED => {
            Some(detail.clone().unwrap_or_default())
        }
        zbus::Error::FDO(error) => match error.as_ref() {
            zbus::fdo::Error::FileExists(detail) => Some(detail.clone()),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_are_read_by_name() {
        assert_eq!(read("restored"), Some(Restored::Restored));
        assert_eq!(read("replaced"), Some(Restored::Replaced));
        assert_eq!(read("unchanged"), Some(Restored::Unchanged));
        assert_eq!(read("deleted"), None);
        assert_eq!(read(""), None);
    }

    #[cfg(feature = "bus")]
    #[test]
    fn only_file_exists_is_a_changed_file() {
        let exists = zbus::Error::FDO(Box::new(zbus::fdo::Error::FileExists(
            "/home/eclipse/notes.txt has changed since this snapshot.".into(),
        )));
        assert_eq!(
            changed(&exists).as_deref(),
            Some("/home/eclipse/notes.txt has changed since this snapshot.")
        );
        let missing = zbus::Error::FDO(Box::new(zbus::fdo::Error::FileNotFound(
            "There is no snapshot 2026-09-12T14:00:03Z.".into(),
        )));
        assert_eq!(changed(&missing), None);
    }
}
