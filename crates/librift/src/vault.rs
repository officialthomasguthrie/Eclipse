//! Vault from a client's side: the snapshots Timeline keeps of home and the backups on the backup
//! disk, making them, and restoring a file from one.

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

/// How long listing the backups may take. Vault mounts the disk and rustic reads the index.
#[cfg(feature = "bus")]
const BACKUPS_TIMEOUT: Duration = Duration::from_secs(600);

/// How long a backup may take. The first one of a full home reads all of it.
#[cfg(feature = "bus")]
const BACKUP_TIMEOUT: Duration = Duration::from_secs(24 * 3600);

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

/// The first 8 digits of a backup's id, which is what people see and type.
#[must_use]
pub fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Seconds since 1970 as the name of a snapshot taken then: `2026-09-12T14:00:03Z`.
#[must_use]
pub fn snapshot_name(secs: i64) -> String {
    let (year, month, day) = civil(secs.div_euclid(DAY));
    let rest = secs.rem_euclid(DAY);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / HOUR,
        rest % HOUR / 60,
        rest % 60
    )
}

/// The time a snapshot name stands for in seconds since 1970, or `None` when the name is not one.
/// Only the exact form [`snapshot_name`] writes counts, so every time has one name.
#[must_use]
pub fn snapshot_time(name: &str) -> Option<i64> {
    let bytes = name.as_bytes();
    let shape = [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'Z'),
    ];
    if bytes.len() != 20 || shape.iter().any(|&(at, c)| bytes[at] != c) {
        return None;
    }
    let number = |from: usize, to: usize| {
        name.get(from..to)?.bytes().try_fold(0, |n: i64, digit| {
            if digit.is_ascii_digit() {
                Some(n * 10 + i64::from(digit - b'0'))
            } else {
                None
            }
        })
    };
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    let valid = (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && hour < 24
        && minute < 60
        && second < 60;
    valid.then(|| days_from_civil(year, month, day) * DAY + hour * HOUR + minute * 60 + second)
}

/// Days since 1970-01-01 of a date, the era arithmetic from Howard Hinnant's notes.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day since 1970-01-01.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

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
    restore_with("Restore", snapshot, path, replace)
}

/// The backups on the backup disk, oldest first: each one's id and when it was made.
///
/// # Errors
///
/// A sentence when the bus or Vault is not there, there is no backup disk, or it is not plugged in.
#[cfg(feature = "bus")]
pub fn backups() -> Result<Vec<(String, String)>, String> {
    let vault = Component::Vault;
    let connection = bus::connect(BACKUPS_TIMEOUT)?;
    let proxy = bus::proxy(&connection, vault)?;
    proxy
        .call("Backups", &())
        .map_err(|e| bus::sentence(vault, e))
}

/// Backs up home now. Returns the backup's id and when it was made.
///
/// # Errors
///
/// A sentence when the bus or Vault is not there, or the backup could not be made.
#[cfg(feature = "bus")]
pub fn backup() -> Result<(String, String), String> {
    let vault = Component::Vault;
    let connection = bus::connect(BACKUP_TIMEOUT)?;
    let proxy = bus::proxy(&connection, vault)?;
    proxy
        .call("Backup", &())
        .map_err(|e| bus::sentence(vault, e))
}

/// Restores `path`, a full path to a file in home, from the backup with the id `backup`, the way
/// [`restore`] does from a snapshot.
///
/// # Errors
///
/// [`Refusal::Changed`] when the file changed and `replace` is false, otherwise a sentence.
#[cfg(feature = "bus")]
pub fn restore_backup(
    backup: &str,
    path: &str,
    replace: bool,
) -> Result<(Restored, String), Refusal> {
    restore_with("RestoreBackup", backup, path, replace)
}

#[cfg(feature = "bus")]
fn restore_with(
    method: &str,
    from: &str,
    path: &str,
    replace: bool,
) -> Result<(Restored, String), Refusal> {
    let vault = Component::Vault;
    let connection = bus::connect(RESTORE_TIMEOUT).map_err(Refusal::Other)?;
    let proxy = bus::proxy(&connection, vault).map_err(Refusal::Other)?;
    let (outcome, written): (String, String) =
        proxy
            .call(method, &(from, path, replace))
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

    #[test]
    fn a_backup_is_shown_by_its_first_eight_digits() {
        assert_eq!(
            short("e863e83c77b4f162be953870ddaaf7ff70f92ab02e129fa01b3752e0495b489d"),
            "e863e83c"
        );
        assert_eq!(short("e863"), "e863");
    }

    #[cfg(feature = "bus")]
    #[test]
    fn only_file_exists_is_a_changed_file() {
        let exists = zbus::Error::FDO(Box::new(zbus::fdo::Error::FileExists(
            "/home/rift/notes.txt has changed since this snapshot.".into(),
        )));
        assert_eq!(
            changed(&exists).as_deref(),
            Some("/home/rift/notes.txt has changed since this snapshot.")
        );
        let missing = zbus::Error::FDO(Box::new(zbus::fdo::Error::FileNotFound(
            "There is no snapshot 2026-09-12T14:00:03Z.".into(),
        )));
        assert_eq!(changed(&missing), None);
    }
}
