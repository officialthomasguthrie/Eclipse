//! Timeline: read-only snapshots of home, each named by the UTC time it was taken, and the rules
//! that decide which of them to keep.
//!
//! The rules keep the first snapshot of each of the latest hours that have one, of the latest
//! days and of the latest weeks, and always the newest. They count periods that have a snapshot,
//! not periods back from now, so a drive that was switched off for a month keeps what it had and a
//! machine with a wrong clock cannot make them delete everything.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rustix::fs::FlockOperation;

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;
const WEEK: i64 = 7 * DAY;
/// 1970-01-01 was a Thursday. Counted from three days later, weeks start on a Monday.
const WEEK_START: i64 = 3 * DAY;

/// How many of the latest hours, days and weeks keep their first snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keep {
    pub hourly: usize,
    pub daily: usize,
    pub weekly: usize,
}

impl Default for Keep {
    fn default() -> Self {
        Keep {
            hourly: 24,
            daily: 7,
            weekly: 8,
        }
    }
}

// a snapshot's name is the UTC time it was taken, `2026-09-12T14:00:03Z`. the rift command reads
// the names too, so librift holds the one way to write and read them
pub use librift::vault::{snapshot_name as name_of, snapshot_time as parse};

/// The times out of `times` that the rules keep.
pub fn kept(times: &[i64], keep: Keep) -> BTreeSet<i64> {
    let all: BTreeSet<i64> = times.iter().copied().collect();
    let mut kept: BTreeSet<i64> = all.last().copied().into_iter().collect();
    for (length, start, count) in [
        (HOUR, 0, keep.hourly),
        (DAY, 0, keep.daily),
        (WEEK, WEEK_START, keep.weekly),
    ] {
        // oldest first, so the first time seen in a period is the one it keeps
        let mut firsts = BTreeMap::new();
        for &time in &all {
            firsts
                .entry((time + start).div_euclid(length))
                .or_insert(time);
        }
        kept.extend(firsts.values().rev().take(count).copied());
    }
    kept
}

/// The names out of `names` that the rules drop, oldest first. A name that is not a time is
/// never dropped, and neither is `taken`, the snapshot the same run just took, even when a wrong
/// clock named it older than the rest.
pub fn to_drop<'a>(names: &'a [String], keep: Keep, taken: Option<&str>) -> Vec<&'a str> {
    let dated: Vec<(i64, &str)> = names
        .iter()
        .filter_map(|name| Some((parse(name)?, name.as_str())))
        .collect();
    let times: Vec<i64> = dated.iter().map(|&(time, _)| time).collect();
    let kept = kept(&times, keep);
    let mut dropped: Vec<(i64, &str)> = dated
        .into_iter()
        .filter(|&(time, name)| !kept.contains(&time) && Some(name) != taken)
        .collect();
    dropped.sort_unstable();
    dropped.into_iter().map(|(_, name)| name).collect()
}

/// Where the snapshots are, what they are of, and how many to keep.
#[derive(Debug, Clone)]
pub struct Timeline {
    /// The subvolume that is snapshotted, `/persist/@home`.
    pub subvolume: PathBuf,
    /// The directory the snapshots are in, `/persist/@snapshots/home`.
    pub snapshots: PathBuf,
    pub keep: Keep,
}

impl Timeline {
    /// Every snapshot, oldest first.
    pub fn list(&self) -> io::Result<Vec<String>> {
        names_in(&self.snapshots)
    }

    /// Takes a read-only snapshot now, then runs the rules. Returns its name and the names the
    /// rules dropped.
    pub fn take(&self) -> Result<(String, Vec<String>), String> {
        let _lock = self.lock()?;
        let mut name = name_of(now());
        if self.snapshots.join(&name).exists() {
            // another snapshot in the same second
            std::thread::sleep(Duration::from_secs(1));
            name = name_of(now());
        }
        let target = self.snapshots.join(&name);
        btrfs([
            OsStr::new("subvolume"),
            OsStr::new("snapshot"),
            OsStr::new("-r"),
            self.subvolume.as_os_str(),
            target.as_os_str(),
        ])?;
        let dropped = self.drop_past(Some(&name))?;
        Ok((name, dropped))
    }

    /// Runs the rules and returns the names dropped.
    pub fn prune(&self) -> Result<Vec<String>, String> {
        let _lock = self.lock()?;
        self.drop_past(None)
    }

    fn drop_past(&self, taken: Option<&str>) -> Result<Vec<String>, String> {
        let names = self
            .list()
            .map_err(|e| format!("Could not read {}: {e}", self.snapshots.display()))?;
        let mut dropped = Vec::new();
        for name in to_drop(&names, self.keep, taken) {
            let path = self.snapshots.join(name);
            btrfs([
                OsStr::new("subvolume"),
                OsStr::new("delete"),
                path.as_os_str(),
            ])?;
            dropped.push(name.to_string());
        }
        Ok(dropped)
    }

    fn lock(&self) -> Result<File, String> {
        lock(&self.snapshots)
    }
}

/// The names of the snapshots in `directory`, oldest first. Anything not named by a time is left
/// out.
pub fn names_in(directory: &Path) -> io::Result<Vec<String>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str().filter(|n| parse(n).is_some()) {
            names.push(name.to_string());
        }
    }
    // names of one fixed width sort the way their times do
    names.sort_unstable();
    Ok(names)
}

/// Holds an flock on `directory` until the file is dropped, so a snapshot from the timer and one
/// from the bus never change the directory at the same time.
pub fn lock(directory: &Path) -> Result<File, String> {
    fs::create_dir_all(directory)
        .and_then(|()| File::open(directory))
        .and_then(|file| {
            rustix::fs::flock(&file, FlockOperation::LockExclusive)?;
            Ok(file)
        })
        .map_err(|e| format!("Could not lock {}: {e}", directory.display()))
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

pub fn btrfs<'a>(args: impl IntoIterator<Item = &'a OsStr>) -> Result<(), String> {
    let args: Vec<&OsStr> = args.into_iter().collect();
    let line = args
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let output = Command::new("btrfs")
        .args(&args)
        .output()
        .map_err(|e| format!("Could not run btrfs: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "btrfs {line} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(name: &str) -> i64 {
        parse(name).unwrap_or_else(|| panic!("{name} is not a snapshot name"))
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_string()).collect()
    }

    fn keep(hourly: usize, daily: usize, weekly: usize) -> Keep {
        Keep {
            hourly,
            daily,
            weekly,
        }
    }

    #[test]
    fn a_name_is_the_time_in_utc() {
        assert_eq!(name_of(0), "1970-01-01T00:00:00Z");
        assert_eq!(name_of(1_789_221_603), "2026-09-12T14:00:03Z");
        assert_eq!(name_of(951_825_600), "2000-02-29T12:00:00Z");
        for secs in [
            0,
            59,
            86_399,
            86_400,
            951_825_600,
            1_789_221_603,
            4_102_444_799,
        ] {
            assert_eq!(parse(&name_of(secs)), Some(secs));
        }
    }

    #[test]
    fn only_the_exact_form_is_a_name() {
        for name in [
            "",
            "latest",
            "2026-09-12T14:00:03",
            "2026-09-12T14:00:03z",
            "2026-09-12 14:00:03Z",
            "2026-09-12T14:00:03+00:00",
            "2026-13-01T00:00:00Z",
            "2026-02-29T00:00:00Z",
            "2026-09-31T00:00:00Z",
            "2026-09-12T24:00:00Z",
            "2026-09-12T14:60:00Z",
            "2026-09-12T14:00:60Z",
            "2026-09-1xT14:00:03Z",
            "+026-09-12T14:00:03Z",
        ] {
            assert_eq!(parse(name), None, "{name:?}");
        }
        assert!(parse("2024-02-29T00:00:00Z").is_some());
    }

    #[test]
    fn nothing_is_kept_of_nothing() {
        assert!(kept(&[], Keep::default()).is_empty());
    }

    #[test]
    fn without_rules_only_the_newest_is_kept() {
        let times = [
            at("2026-09-12T10:00:00Z"),
            at("2026-09-12T12:00:00Z"),
            at("2026-09-12T11:00:00Z"),
        ];
        assert_eq!(
            kept(&times, keep(0, 0, 0)),
            BTreeSet::from([at("2026-09-12T12:00:00Z")])
        );
    }

    #[test]
    fn each_hour_keeps_its_first_snapshot() {
        let all = names(&[
            "2026-09-12T09:00:00Z",
            "2026-09-12T10:00:00Z",
            "2026-09-12T10:30:00Z",
            "2026-09-12T11:00:00Z",
            "2026-09-12T11:15:00Z",
            "2026-09-12T11:45:00Z",
        ]);
        // hours 11 and 10 keep their first, hour 9 is past the limit, 11:45 is the newest
        assert_eq!(
            to_drop(&all, keep(2, 0, 0), None),
            [
                "2026-09-12T09:00:00Z",
                "2026-09-12T10:30:00Z",
                "2026-09-12T11:15:00Z"
            ]
        );
    }

    #[test]
    fn periods_without_a_snapshot_do_not_count() {
        // a drive that was off for weeks. the two hours it was on last are the latest two hours
        let all = names(&[
            "2026-08-01T08:00:00Z",
            "2026-08-01T09:00:00Z",
            "2026-09-12T14:00:00Z",
        ]);
        assert_eq!(to_drop(&all, keep(2, 0, 0), None), ["2026-08-01T08:00:00Z"]);
    }

    #[test]
    fn each_day_keeps_its_first_snapshot() {
        let all = names(&[
            "2026-09-10T23:00:00Z",
            "2026-09-11T00:00:00Z",
            "2026-09-11T13:00:00Z",
            "2026-09-12T08:00:00Z",
            "2026-09-12T09:00:00Z",
        ]);
        assert_eq!(
            to_drop(&all, keep(0, 2, 0), None),
            ["2026-09-10T23:00:00Z", "2026-09-11T13:00:00Z"]
        );
    }

    #[test]
    fn weeks_start_on_monday() {
        // 2026-09-06 is a Sunday, 2026-09-07 the Monday after it
        let all = names(&[
            "2026-09-06T23:59:59Z",
            "2026-09-07T00:00:00Z",
            "2026-09-09T12:00:00Z",
            "2026-09-12T12:00:00Z",
        ]);
        assert_eq!(
            to_drop(&all, keep(0, 0, 1), None),
            ["2026-09-06T23:59:59Z", "2026-09-09T12:00:00Z"]
        );
        assert_eq!(to_drop(&all, keep(0, 0, 2), None), ["2026-09-09T12:00:00Z"]);
    }

    #[test]
    fn the_rules_add_up() {
        // the boot test's case: today's snapshots in this hour, and some from January that the
        // test names by hand. 2026-01-05 and 2026-01-12 are Mondays
        let all = names(&[
            "2026-01-05T09:00:00Z",
            "2026-01-12T09:00:00Z",
            "2026-01-13T09:00:00Z",
            "2026-01-13T10:00:00Z",
            "2026-09-12T14:00:03Z",
            "2026-09-12T14:01:10Z",
            "2026-09-12T14:02:20Z",
        ]);
        assert_eq!(
            to_drop(&all, keep(1, 1, 2), None),
            [
                "2026-01-05T09:00:00Z",
                "2026-01-13T09:00:00Z",
                "2026-01-13T10:00:00Z",
                "2026-09-12T14:01:10Z"
            ]
        );
        // the defaults keep every hour, day and week, so only the middle one of this hour goes
        assert_eq!(
            to_drop(&all, Keep::default(), None),
            ["2026-09-12T14:01:10Z"]
        );
    }

    #[test]
    fn a_long_timeline_keeps_what_the_defaults_say() {
        // one snapshot an hour for sixty days
        let start = at("2026-07-01T00:00:00Z");
        let all: Vec<String> = (0..60 * 24).map(|h| name_of(start + h * HOUR)).collect();
        let dropped = to_drop(&all, Keep::default(), None);
        let left: Vec<&String> = all
            .iter()
            .filter(|name| !dropped.contains(&name.as_str()))
            .collect();
        // 24 hours, the first of the 6 days before them, and the Mondays of the 8 weeks before
        // those that are not already kept. 2026-08-29 is the last day, a Saturday
        assert_eq!(left.len(), 24 + 6 + 7, "{left:?}");
        assert_eq!(
            left.first().map(|n| n.as_str()),
            Some("2026-07-06T00:00:00Z")
        );
        assert_eq!(
            left.last().map(|n| n.as_str()),
            Some("2026-08-29T23:00:00Z")
        );
    }

    #[test]
    fn names_that_are_not_times_and_the_one_just_taken_stay() {
        let all = names(&[
            "notes",
            "2026-09-12T14:00:00Z",
            "2026-09-12T14:30:00Z",
            "2020-01-01T00:00:00Z",
        ]);
        // a clock years behind named the new snapshot 2020. the run that took it keeps it
        assert_eq!(
            to_drop(&all, keep(1, 0, 0), Some("2020-01-01T00:00:00Z")),
            Vec::<&str>::new()
        );
        assert_eq!(to_drop(&all, keep(1, 0, 0), None), ["2020-01-01T00:00:00Z"]);
    }
}
