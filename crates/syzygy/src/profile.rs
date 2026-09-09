//! The host profile: a small TOML file per fingerprint under the hosts directory, plus a
//! `current` file naming the one for the machine we are on.
//!
//! Only the subset of TOML written here is read back: `key = "string"`, `key = ["a", "b"]` and
//! one `[dmi]` table. That keeps the crate dependency free.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::host::{DMI_FIELDS, Host};

/// The default class for a machine seen for the first time.
pub const DEFAULT_CLASS: &str = "borrowed";

/// Everything the profile file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Hex SHA-256 from [`Host::fingerprint`].
    pub fingerprint: String,
    /// `owned`, `trusted` or `borrowed`.
    pub class: String,
    /// UTC, `2026-09-09T18:30:00Z`.
    pub first_seen: String,
    /// UTC, same format.
    pub last_seen: String,
    /// What the fingerprint was computed from.
    pub host: Host,
}

impl Profile {
    /// A fresh profile for `host`, first and last seen `now`.
    #[must_use]
    pub fn new(host: Host, now: &str) -> Self {
        Self {
            fingerprint: host.fingerprint(),
            class: DEFAULT_CLASS.to_owned(),
            first_seen: now.to_owned(),
            last_seen: now.to_owned(),
            host,
        }
    }

    /// The file contents.
    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "# Eclipse host profile, written by Syzygy. Settings here override the defaults.\n",
        );
        let _ = writeln!(out, "fingerprint = {}", quote(&self.fingerprint));
        let _ = writeln!(out, "class = {}", quote(&self.class));
        let _ = writeln!(out, "first_seen = {}", quote(&self.first_seen));
        let _ = writeln!(out, "last_seen = {}", quote(&self.last_seen));
        let pci: Vec<String> = self.host.pci.iter().map(|id| quote(id)).collect();
        let _ = writeln!(out, "pci = [{}]", pci.join(", "));
        out.push_str("\n[dmi]\n");
        for (name, value) in self.host.dmi_fields() {
            let _ = writeln!(out, "{name} = {}", quote(value));
        }
        out
    }

    /// Reads a profile written by [`Profile::to_toml`].
    ///
    /// # Errors
    ///
    /// When a line is not one of the forms this crate writes, or the fingerprint is missing.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut profile = Self {
            fingerprint: String::new(),
            class: DEFAULT_CLASS.to_owned(),
            first_seen: String::new(),
            last_seen: String::new(),
            host: Host::default(),
        };
        let mut in_dmi = false;
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line == "[dmi]" {
                in_dmi = true;
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("line {}: expected key = value", n + 1));
            };
            let (key, value) = (key.trim(), strip_comment(value).trim());
            let bad = |what: &str| format!("line {}: {what}", n + 1);
            if in_dmi {
                if let Some(i) = DMI_FIELDS.iter().position(|f| *f == key) {
                    profile.host.dmi[i] = unquote(value).ok_or_else(|| bad("expected a string"))?;
                }
                continue;
            }
            match key {
                "fingerprint" => {
                    profile.fingerprint = unquote(value).ok_or_else(|| bad("expected a string"))?;
                }
                "class" => {
                    profile.class = unquote(value).ok_or_else(|| bad("expected a string"))?;
                }
                "first_seen" => {
                    profile.first_seen = unquote(value).ok_or_else(|| bad("expected a string"))?;
                }
                "last_seen" => {
                    profile.last_seen = unquote(value).ok_or_else(|| bad("expected a string"))?;
                }
                "pci" => {
                    profile.host.pci = unquote_list(value).ok_or_else(|| bad("expected a list"))?;
                }
                _ => {}
            }
        }
        if profile.fingerprint.is_empty() {
            return Err("no fingerprint in the profile".to_owned());
        }
        Ok(profile)
    }
}

/// What [`record`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    /// No profile existed, one was written.
    New,
    /// A profile existed, only its `last_seen` changed.
    Again,
}

/// Where the profile for `fingerprint` lives under `hosts_dir`.
#[must_use]
pub fn path(hosts_dir: &Path, fingerprint: &str) -> PathBuf {
    hosts_dir.join(format!("{fingerprint}.toml"))
}

/// Reads the stored profile for `fingerprint`, `None` when there is none yet.
///
/// # Errors
///
/// I/O errors other than a missing file, and a file that does not parse (as `InvalidData`).
pub fn load(hosts_dir: &Path, fingerprint: &str) -> io::Result<Option<Profile>> {
    match fs::read_to_string(path(hosts_dir, fingerprint)) {
        Ok(text) => Profile::parse(&text)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Writes or updates the profile for `host` under `hosts_dir` and points `current` at it.
/// An existing profile keeps everything but `last_seen`, so edits to it survive.
///
/// # Errors
///
/// Any I/O error from reading or writing under `hosts_dir`.
pub fn record(hosts_dir: &Path, host: &Host, now: &str) -> io::Result<(PathBuf, Seen)> {
    let fingerprint = host.fingerprint();
    let path = path(hosts_dir, &fingerprint);
    let seen = match fs::read_to_string(&path) {
        Ok(existing) => {
            write_atomically(&path, &set_last_seen(&existing, now))?;
            Seen::Again
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            write_atomically(&path, &Profile::new(host.clone(), now).to_toml())?;
            Seen::New
        }
        Err(e) => return Err(e),
    };
    write_atomically(&hosts_dir.join("current"), &format!("{fingerprint}\n"))?;
    Ok((path, seen))
}

/// Replaces the `last_seen` line, or adds one after `first_seen` when the file has none.
fn set_last_seen(text: &str, now: &str) -> String {
    let line = format!("last_seen = {}\n", quote(now));
    let key_of = |raw: &str| raw.split_once('=').map(|(k, _)| k.trim().to_owned());
    let has_last_seen = text
        .lines()
        .any(|l| key_of(l).as_deref() == Some("last_seen"));
    let mut out = String::with_capacity(text.len() + line.len());
    let mut done = false;
    for raw in text.split_inclusive('\n') {
        let key = key_of(raw);
        if !done && key.as_deref() == Some("last_seen") {
            out.push_str(&line);
            done = true;
            continue;
        }
        out.push_str(raw);
        if !done && !has_last_seen && key.as_deref() == Some("first_seen") {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&line);
            done = true;
        }
    }
    if !done {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&line);
    }
    out
}

fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)
}

/// The current time as `2026-09-09T18:30:00Z`.
#[must_use]
pub fn now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    timestamp(secs)
}

/// Seconds since the epoch as an RFC 3339 UTC timestamp, no fractional part.
#[must_use]
pub fn timestamp(secs: u64) -> String {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3600, rem % 3600 / 60, rem % 60);
    // civil date from days since 1970-01-01, the era arithmetic from Howard Hinnant's notes
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// A TOML basic string.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Cuts a `# comment` off the end of a value, leaving `#` inside quotes alone.
fn strip_comment(value: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in value.char_indices() {
        match c {
            '\\' if in_string => escaped = !escaped,
            '"' if !escaped => in_string = !in_string,
            '#' if !in_string => return &value[..i],
            _ => escaped = false,
        }
        if c != '\\' {
            escaped = false;
        }
    }
    value
}

fn unquote(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'u' => {
                let hex: String = chars.by_ref().take(4).collect();
                let code = u32::from_str_radix(&hex, 16).ok()?;
                out.push(char::from_u32(code)?);
            }
            _ => return None,
        }
    }
    Some(out)
}

fn unquote_list(value: &str) -> Option<Vec<String>> {
    let inner = value.strip_prefix('[')?.strip_suffix(']')?.trim();
    if inner.is_empty() {
        return Some(Vec::new());
    }
    inner.split(',').map(|item| unquote(item.trim())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_host() -> Host {
        Host {
            dmi: [
                "QEMU".into(),
                "Standard PC (Q35 + ICH9, 2009)".into(),
                "pc-q35-9.0".into(),
                String::new(),
                "a \"quoted\" board\\name".into(),
                "rel-1.16.3".into(),
            ],
            pci: vec!["1234:1111".into(), "8086:29c0".into()],
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("syzygy-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn toml_round_trip() {
        let profile = Profile::new(sample_host(), "2026-09-09T18:30:00Z");
        let text = profile.to_toml();
        assert!(text.contains("class = \"borrowed\""));
        assert!(text.contains("pci = [\"1234:1111\", \"8086:29c0\"]"));
        assert!(text.contains("board_name = \"a \\\"quoted\\\" board\\\\name\""));
        assert_eq!(Profile::parse(&text).unwrap(), profile);
    }

    #[test]
    fn parse_rejects_junk() {
        assert!(Profile::parse("").is_err());
        assert!(Profile::parse("fingerprint = 12").is_err());
        assert!(Profile::parse("what is this").is_err());
    }

    #[test]
    fn first_boot_then_second_boot() {
        let dir = temp_dir("record");
        let host = sample_host();

        let (path, seen) = record(&dir, &host, "2026-09-09T18:30:00Z").unwrap();
        assert_eq!(seen, Seen::New);
        assert_eq!(path, dir.join(format!("{}.toml", host.fingerprint())));
        let current = fs::read_to_string(dir.join("current")).unwrap();
        assert_eq!(current.trim(), host.fingerprint());
        let first = Profile::parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(first.first_seen, "2026-09-09T18:30:00Z");
        assert_eq!(first.class, "borrowed");

        // a person edited the class in between; the edit and the comment stay
        let edited = fs::read_to_string(&path)
            .unwrap()
            .replace("class = \"borrowed\"", "class = \"owned\" # mine");
        fs::write(&path, edited).unwrap();

        let (_, seen) = record(&dir, &host, "2026-09-10T08:00:00Z").unwrap();
        assert_eq!(seen, Seen::Again);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("class = \"owned\" # mine"));
        let second = Profile::parse(&text).unwrap();
        assert_eq!(second.first_seen, "2026-09-09T18:30:00Z");
        assert_eq!(second.last_seen, "2026-09-10T08:00:00Z");
        assert_eq!(second.host, host);
        assert!(!dir.join(format!("{}.tmp", host.fingerprint())).exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn last_seen_is_added_when_missing() {
        let text = "fingerprint = \"ab\"\nfirst_seen = \"x\"\npci = []\n";
        let out = set_last_seen(text, "y");
        assert_eq!(
            out,
            "fingerprint = \"ab\"\nfirst_seen = \"x\"\nlast_seen = \"y\"\npci = []\n"
        );
        assert_eq!(
            set_last_seen("fingerprint = \"ab\"", "y"),
            "fingerprint = \"ab\"\nlast_seen = \"y\"\n"
        );
    }

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(timestamp(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(timestamp(4_102_444_800), "2100-01-01T00:00:00Z");
        assert_eq!(now().len(), 20);
    }

    #[test]
    fn comments_after_values() {
        assert_eq!(strip_comment("\"a\" # b"), "\"a\" ");
        assert_eq!(strip_comment("\"a # b\""), "\"a # b\"");
        assert_eq!(strip_comment("\"a\\\" # b\" # c"), "\"a\\\" # b\" ");
        assert_eq!(strip_comment("[\"x\"]"), "[\"x\"]");
    }

    #[test]
    fn quoting() {
        for s in [
            "",
            "plain",
            "a \"b\" c",
            "back\\slash",
            "tab\tnew\nline",
            "\u{1}bell",
        ] {
            assert_eq!(unquote(&quote(s)).as_deref(), Some(s));
        }
        assert_eq!(unquote("unterminated"), None);
        assert_eq!(unquote("\"bad \\x\""), None);
    }
}
