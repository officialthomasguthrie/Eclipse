//! The host profile: a small TOML file per fingerprint under the hosts directory, plus a
//! `current` file naming the one for the machine we are on.
//!
//! The file is a delta, not a dump. `[detected]` is what Orbit worked out about this machine
//! and holds only what differs from the built in defaults, so it stays a few lines and a change
//! to a default reaches every machine on its next boot. `[set]` is what a person or another
//! Rift service decided; Orbit never writes into it and copies it through every rewrite,
//! comments and all. Effective values are the defaults, then `[detected]`, then `[set]`.
//!
//! Only the subset of TOML written here is read back: `key = "string"`, `key = 12`, the two
//! tables and their `display` arrays. That keeps the crate dependency free.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::displays::{self, Display};
use crate::gpu;
use crate::host::{self, Host};

/// The default class for a machine seen for the first time.
pub const DEFAULT_CLASS: &str = "borrowed";

/// Every setting a host profile can carry, with the value the whole fleet gets when the file
/// says nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// `owned`, `trusted` or `borrowed`. Only a person sets this.
    pub class: String,
    /// `laptop` or `desktop`.
    pub chassis: String,
    /// `intel`, `amd`, `nvidia` or `unknown`.
    pub gpu_vendor: String,
    /// `mesa`, `nvk` or `none`.
    pub gpu_path: String,
    /// Which Quasar model tier this machine can carry.
    pub ai_tier: String,
    /// One entry per connected output.
    pub displays: Vec<Display>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            class: DEFAULT_CLASS.to_owned(),
            chassis: host::DESKTOP.to_owned(),
            // there is no majority vendor, so an unread card stays unnamed
            gpu_vendor: gpu::UNKNOWN_VENDOR.to_owned(),
            // decision record 0012: the open drivers unless the machine says otherwise
            gpu_path: gpu::MESA.to_owned(),
            ai_tier: host::TIERS[1].to_owned(),
            displays: Vec::new(),
        }
    }
}

impl Settings {
    /// Only the settings that differ from the defaults.
    #[must_use]
    pub fn delta(&self) -> Layer {
        let d = Self::default();
        let differs = |a: &String, b: &String| (a != b).then(|| a.clone());
        Layer {
            class: differs(&self.class, &d.class),
            chassis: differs(&self.chassis, &d.chassis),
            gpu_vendor: differs(&self.gpu_vendor, &d.gpu_vendor),
            gpu_path: differs(&self.gpu_path, &d.gpu_path),
            ai_tier: differs(&self.ai_tier, &d.ai_tier),
            displays: (!self.displays.is_empty()).then(|| self.displays.clone()),
        }
    }
}

/// A partial [`Settings`]: what one layer of the file says, `None` for everything it leaves out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layer {
    /// See [`Settings::class`].
    pub class: Option<String>,
    /// See [`Settings::chassis`].
    pub chassis: Option<String>,
    /// See [`Settings::gpu_vendor`].
    pub gpu_vendor: Option<String>,
    /// See [`Settings::gpu_path`].
    pub gpu_path: Option<String>,
    /// See [`Settings::ai_tier`].
    pub ai_tier: Option<String>,
    /// See [`Settings::displays`]. A layer that names any output replaces the whole list.
    pub displays: Option<Vec<Display>>,
}

impl Layer {
    /// This layer laid over `base`. Everything this layer names wins.
    #[must_use]
    pub fn over(&self, base: Settings) -> Settings {
        Settings {
            class: self.class.clone().unwrap_or(base.class),
            chassis: self.chassis.clone().unwrap_or(base.chassis),
            gpu_vendor: self.gpu_vendor.clone().unwrap_or(base.gpu_vendor),
            gpu_path: self.gpu_path.clone().unwrap_or(base.gpu_path),
            ai_tier: self.ai_tier.clone().unwrap_or(base.ai_tier),
            displays: self.displays.clone().unwrap_or(base.displays),
        }
    }

    /// Nothing to write.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.class.is_none()
            && self.chassis.is_none()
            && self.gpu_vendor.is_none()
            && self.gpu_path.is_none()
            && self.ai_tier.is_none()
            && self.displays.is_none()
    }

    /// This layer as the TOML table `name`, empty when there is nothing in it.
    #[must_use]
    pub fn to_toml(&self, name: &str) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut out = format!("[{name}]\n");
        for (key, value) in [
            ("class", &self.class),
            ("chassis", &self.chassis),
            ("gpu_vendor", &self.gpu_vendor),
            ("gpu_path", &self.gpu_path),
            ("ai_tier", &self.ai_tier),
        ] {
            if let Some(value) = value {
                let _ = writeln!(out, "{key} = {}", quote(value));
            }
        }
        for display in self.displays.iter().flatten() {
            let _ = write!(out, "\n[[{name}.display]]\n{}", display_to_toml(display));
        }
        out
    }
}

/// One output as a table body, leaving out everything it does not know.
fn display_to_toml(display: &Display) -> String {
    let mut out = format!("connector = {}\n", quote(&display.connector));
    if display.mode != (0, 0) {
        let _ = writeln!(
            out,
            "width = {}\nheight = {}",
            display.mode.0, display.mode.1
        );
    }
    if display.size_cm != (0, 0) {
        let _ = writeln!(
            out,
            "width_cm = {}\nheight_cm = {}",
            display.size_cm.0, display.size_cm.1
        );
    }
    if display.scale != Display::default().scale {
        let _ = writeln!(out, "scale = {}", display.scale);
    }
    out
}

/// What a machine is, apart from its settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    /// Hex SHA-256 from [`Host::fingerprint`].
    pub fingerprint: String,
    /// The machine in words, for whoever opens the file.
    pub host: String,
    /// UTC, `2026-09-09T18:30:00Z`.
    pub first_seen: String,
    /// UTC, same format.
    pub last_seen: String,
}

/// A machine and the settings that came out of the defaults, the detection and the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    /// Which machine this is.
    pub identity: Identity,
    /// What the rest of the system should use.
    pub settings: Settings,
}

impl Profile {
    /// Every effective value, spelled out. This is what `orbit --print` shows, and it is not
    /// the file: the file is the delta.
    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut out = String::from("# The effective profile for this machine.\n");
        out.push_str(&identity_to_toml(&self.identity));
        let mut every = Layer {
            class: Some(self.settings.class.clone()),
            chassis: Some(self.settings.chassis.clone()),
            gpu_vendor: Some(self.settings.gpu_vendor.clone()),
            gpu_path: Some(self.settings.gpu_path.clone()),
            ai_tier: Some(self.settings.ai_tier.clone()),
            displays: None,
        };
        if !self.settings.displays.is_empty() {
            every.displays = Some(self.settings.displays.clone());
        }
        out.push('\n');
        out.push_str(&every.to_toml("profile"));
        out
    }
}

/// What a stored profile file holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stored {
    /// The keys outside the two tables.
    pub identity: Identity,
    /// The `[set]` table, read.
    pub set: Layer,
    /// The `[set]` table exactly as the file has it, so comments and spacing survive a rewrite.
    pub set_text: String,
}

impl Stored {
    /// Reads a profile file.
    ///
    /// # Errors
    ///
    /// When a line is not one of the forms this crate writes, or the fingerprint is missing.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut stored = Self::default();
        let mut section = Section::Root;
        let mut set_displays: Vec<Display> = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if let Some(header) = table_header(line) {
                section = header;
                if section == Section::SetDisplay {
                    set_displays.push(Display::default());
                }
                if section.is_set() {
                    let _ = writeln!(stored.set_text, "{raw}");
                }
                continue;
            }
            if section.is_set() {
                let _ = writeln!(stored.set_text, "{raw}");
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("line {}: expected key = value", n + 1));
            };
            let (key, value) = (key.trim(), strip_comment(value).trim());
            let bad = |what: &str| format!("line {}: {what}", n + 1);
            match section {
                Section::Root => stored.identity.set(key, value).map_err(bad)?,
                Section::Set => stored.set.set(key, value).map_err(bad)?,
                Section::SetDisplay => set_displays
                    .last_mut()
                    .ok_or_else(|| bad("a display outside a display table"))?
                    .set(key, value)
                    .map_err(bad)?,
                Section::Other => {}
            }
        }
        if !set_displays.is_empty() {
            stored.set.displays = Some(set_displays);
        }
        if stored.identity.fingerprint.is_empty() {
            return Err("no fingerprint in the profile".to_owned());
        }
        Ok(stored)
    }
}

impl Identity {
    fn set(&mut self, key: &str, value: &str) -> Result<(), &'static str> {
        let field = match key {
            "fingerprint" => &mut self.fingerprint,
            "host" => &mut self.host,
            "first_seen" => &mut self.first_seen,
            "last_seen" => &mut self.last_seen,
            _ => return Ok(()),
        };
        *field = unquote(value).ok_or("expected a string")?;
        Ok(())
    }
}

impl Layer {
    fn set(&mut self, key: &str, value: &str) -> Result<(), &'static str> {
        let field = match key {
            "class" => &mut self.class,
            "chassis" => &mut self.chassis,
            "gpu_vendor" => &mut self.gpu_vendor,
            "gpu_path" => &mut self.gpu_path,
            "ai_tier" => &mut self.ai_tier,
            _ => return Ok(()),
        };
        *field = Some(unquote(value).ok_or("expected a string")?);
        Ok(())
    }
}

impl Display {
    fn set(&mut self, key: &str, value: &str) -> Result<(), &'static str> {
        if key == "connector" {
            self.connector = unquote(value).ok_or("expected a string")?;
            return Ok(());
        }
        let number: u32 = match key {
            "width" | "height" | "width_cm" | "height_cm" | "scale" => {
                value.parse().map_err(|_| "expected a number")?
            }
            _ => return Ok(()),
        };
        match key {
            "width" => self.mode.0 = number,
            "height" => self.mode.1 = number,
            "width_cm" => self.size_cm.0 = number,
            "height_cm" => self.size_cm.1 = number,
            _ => self.scale = number,
        }
        Ok(())
    }
}

/// Which part of the file the reader is in. Only the root and `[set]` are read back; `[detected]`
/// is rewritten from the machine every boot, so what the file says about it does not matter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Root,
    Set,
    SetDisplay,
    Other,
}

impl Section {
    const fn is_set(self) -> bool {
        matches!(self, Section::Set | Section::SetDisplay)
    }
}

/// The section a `[table]` or `[[table.array]]` line starts, `None` when the line is not a header.
fn table_header(line: &str) -> Option<Section> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    let path = inner
        .strip_prefix('[')
        .map_or(inner, |rest| rest.strip_suffix(']').unwrap_or(rest));
    Some(match path.trim() {
        "set" => Section::Set,
        "set.display" => Section::SetDisplay,
        _ => Section::Other,
    })
}

/// What [`record`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    /// No profile existed, one was written.
    New,
    /// A profile existed, it was brought up to date.
    Again,
}

/// Where the profile for `fingerprint` lives under `hosts_dir`.
#[must_use]
pub fn path(hosts_dir: &Path, fingerprint: &str) -> PathBuf {
    hosts_dir.join(format!("{fingerprint}.toml"))
}

/// Reads the machine: the graphics path, the outputs, the chassis and the AI tier.
#[must_use]
pub fn detect() -> Settings {
    let (gpu_vendor, gpu_path) = gpu::detect();
    Settings {
        class: DEFAULT_CLASS.to_owned(),
        chassis: host::chassis().to_owned(),
        gpu_vendor,
        gpu_path,
        ai_tier: host::ai_tier().to_owned(),
        displays: displays::detect(),
    }
}

/// Reads the stored profile for `fingerprint`, `None` when there is none yet.
///
/// # Errors
///
/// I/O errors other than a missing file, and a file that does not parse (as `InvalidData`).
pub fn load(hosts_dir: &Path, fingerprint: &str) -> io::Result<Option<Stored>> {
    match fs::read_to_string(path(hosts_dir, fingerprint)) {
        Ok(text) => Stored::parse(&text)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Writes the profile for `host` under `hosts_dir`, points `current` at it, and gives back the
/// effective profile. `[set]` from an existing file is carried through untouched.
///
/// # Errors
///
/// Any I/O error from reading or writing under `hosts_dir`, and a stored file that does not parse.
pub fn record(
    hosts_dir: &Path,
    host: &Host,
    detected: &Settings,
    now: &str,
) -> io::Result<(PathBuf, Seen, Profile)> {
    let fingerprint = host.fingerprint();
    let path = path(hosts_dir, &fingerprint);
    let (stored, seen) = match load(hosts_dir, &fingerprint)? {
        Some(stored) => (stored, Seen::Again),
        None => (Stored::default(), Seen::New),
    };
    let identity = Identity {
        fingerprint: fingerprint.clone(),
        host: host.label(),
        first_seen: if stored.identity.first_seen.is_empty() {
            now.to_owned()
        } else {
            stored.identity.first_seen.clone()
        },
        last_seen: now.to_owned(),
    };
    write_atomically(&path, &render(&identity, detected, &stored.set_text))?;
    write_atomically(&hosts_dir.join("current"), &format!("{fingerprint}\n"))?;
    Ok((
        path,
        seen,
        Profile {
            settings: stored.set.over(detected.clone()),
            identity,
        },
    ))
}

/// The file: the identity, the detected delta, then the `[set]` block as it was.
#[must_use]
pub fn render(identity: &Identity, detected: &Settings, set_text: &str) -> String {
    let mut out = String::from(
        "# Rift host profile, written by Orbit.\n\
         # [detected] is what Orbit worked out about this machine. It is rewritten on every\n\
         # boot and holds only what differs from the defaults.\n\
         # Put your own settings under [set]. They win, and Orbit leaves them alone.\n",
    );
    out.push_str(&identity_to_toml(identity));
    let delta = detected.delta().to_toml("detected");
    if !delta.is_empty() {
        out.push('\n');
        out.push_str(&delta);
    }
    let set_text = set_text.trim_end();
    if !set_text.is_empty() {
        out.push('\n');
        out.push_str(set_text);
        out.push('\n');
    }
    out
}

fn identity_to_toml(identity: &Identity) -> String {
    let mut out = String::new();
    for (key, value) in [
        ("fingerprint", &identity.fingerprint),
        ("host", &identity.host),
        ("first_seen", &identity.first_seen),
        ("last_seen", &identity.last_seen),
    ] {
        let _ = writeln!(out, "{key} = {}", quote(value));
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

    /// A machine with no card we have a path for, an output that gives no EDID, and 4 GB of
    /// memory. A KVM switch and a cheap adapter both look like this.
    fn no_edid() -> Settings {
        Settings {
            gpu_vendor: gpu::UNKNOWN_VENDOR.to_owned(),
            gpu_path: gpu::NONE.to_owned(),
            ai_tier: "small".to_owned(),
            displays: vec![Display {
                connector: "Virtual-1".to_owned(),
                ..Display::default()
            }],
            ..Settings::default()
        }
    }

    /// What the boot test's qemu machine really is: no pci vendor behind the virtio card, 4 GB
    /// of memory, and one virtual output whose EDID says 1280x800 at 32 by 20 centimetres,
    /// which is about 102 dpi and so scale 1.
    fn qemu() -> Settings {
        Settings {
            gpu_vendor: gpu::UNKNOWN_VENDOR.to_owned(),
            gpu_path: gpu::NONE.to_owned(),
            ai_tier: "small".to_owned(),
            displays: vec![Display {
                connector: "Virtual-1".to_owned(),
                mode: (1280, 800),
                size_cm: (32, 20),
                scale: displays::scale_for((1280, 800), (32, 20)),
            }],
            ..Settings::default()
        }
    }

    /// A laptop with a dense panel and an Intel card.
    fn laptop() -> Settings {
        Settings {
            class: DEFAULT_CLASS.to_owned(),
            chassis: host::LAPTOP.to_owned(),
            gpu_vendor: "intel".to_owned(),
            gpu_path: gpu::MESA.to_owned(),
            ai_tier: "medium".to_owned(),
            displays: vec![Display {
                connector: "eDP-1".to_owned(),
                mode: (2880, 1800),
                size_cm: (30, 19),
                scale: 2,
            }],
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbit-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn identity() -> Identity {
        Identity {
            fingerprint: "ab".repeat(32),
            host: "QEMU Standard PC (Q35 + ICH9, 2009)".to_owned(),
            first_seen: "2026-09-09T18:30:00Z".to_owned(),
            last_seen: "2026-09-10T08:00:00Z".to_owned(),
        }
    }

    #[test]
    fn the_defaults_write_nothing() {
        assert!(Settings::default().delta().is_empty());
        let text = render(&identity(), &Settings::default(), "");
        assert!(!text.contains("\n[detected]\n"), "{text}");
        assert!(!text.contains("class ="), "{text}");
        // four keys and the header comment, nothing else
        assert_eq!(text.lines().filter(|l| l.contains(" = ")).count(), 4);
    }

    #[test]
    fn only_what_differs_is_written() {
        let text = render(&identity(), &no_edid(), "");
        assert!(
            text.contains("[detected]\ngpu_path = \"none\"\nai_tier = \"small\"\n"),
            "{text}"
        );
        // the vendor, the class and the chassis are all the defaults, so they stay out
        assert!(!text.contains("gpu_vendor ="), "{text}");
        assert!(!text.contains("chassis ="), "{text}");
        assert!(!text.contains("class ="), "{text}");
        // an output with no edid is one line: the rest of it is unknown, not zero
        assert!(
            text.contains("[[detected.display]]\nconnector = \"Virtual-1\"\n"),
            "{text}"
        );
        assert!(!text.contains("width ="), "{text}");
        assert!(!text.contains("scale ="), "{text}");
        assert!(
            text.lines().count() < 20,
            "a profile is a few lines: {text}"
        );
    }

    #[test]
    fn the_profile_the_boot_test_reads() {
        let text = render(&identity(), &qemu(), "");
        assert_eq!(qemu().displays[0].scale, 1);
        assert_eq!(text.lines().filter(|l| l.contains(" = ")).count(), 11);
        assert!(!text.contains("scale ="), "{text}");
        assert!(!text.contains("class ="), "{text}");
        assert!(!text.contains("chassis ="), "{text}");
        assert!(!text.contains("gpu_vendor ="), "{text}");
        assert!(text.contains("gpu_path = \"none\""), "{text}");
        assert!(text.contains("ai_tier = \"small\""), "{text}");
        assert!(
            text.contains(
                "[[detected.display]]\nconnector = \"Virtual-1\"\nwidth = 1280\nheight = 800\n\
                 width_cm = 32\nheight_cm = 20\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_laptop_writes_its_panel() {
        let text = render(&identity(), &laptop(), "");
        assert!(text.contains("chassis = \"laptop\""), "{text}");
        assert!(text.contains("gpu_vendor = \"intel\""), "{text}");
        // mesa is the default path, so it is not repeated per machine
        assert!(!text.contains("gpu_path ="), "{text}");
        assert!(!text.contains("ai_tier ="), "{text}");
        assert!(
            text.contains(
                "[[detected.display]]\nconnector = \"eDP-1\"\nwidth = 2880\nheight = 1800\n\
                 width_cm = 30\nheight_cm = 19\nscale = 2\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn set_wins_over_detected_and_over_the_defaults() {
        let text = render(
            &identity(),
            &no_edid(),
            "[set]\nclass = \"owned\" # mine\ngpu_path = \"mesa\"\n",
        );
        let stored = Stored::parse(&text).unwrap();
        assert_eq!(stored.identity, identity());
        assert_eq!(stored.set.class.as_deref(), Some("owned"));
        assert_eq!(stored.set.gpu_path.as_deref(), Some("mesa"));
        let effective = stored.set.over(no_edid());
        assert_eq!(effective.class, "owned");
        assert_eq!(effective.gpu_path, "mesa");
        assert_eq!(effective.ai_tier, "small");
        assert_eq!(effective.chassis, host::DESKTOP);
    }

    #[test]
    fn a_set_display_replaces_the_list() {
        let text = render(
            &identity(),
            &laptop(),
            "[set]\n\n[[set.display]]\nconnector = \"eDP-1\"\nscale = 1\n",
        );
        let stored = Stored::parse(&text).unwrap();
        let effective = stored.set.over(laptop());
        assert_eq!(effective.displays.len(), 1);
        assert_eq!(effective.displays[0].scale, 1);
        // the layer says nothing about the mode, so an unnamed field is unknown, not the panel's
        assert_eq!(effective.displays[0].mode, (0, 0));
    }

    #[test]
    fn parse_rejects_junk() {
        assert!(Stored::parse("").is_err());
        assert!(Stored::parse("fingerprint = 12").is_err());
        assert!(Stored::parse("what is this").is_err());
        assert!(Stored::parse("fingerprint = \"ab\"\n[set]\nclass = 4\n").is_err());
        assert!(Stored::parse("fingerprint = \"ab\"\n[[set.display]]\nwidth = wide\n").is_err());
    }

    #[test]
    fn first_boot_then_second_boot() {
        let dir = temp_dir("record");
        let host = sample_host();
        let detected = no_edid();

        let (path, seen, profile) = record(&dir, &host, &detected, "2026-09-09T18:30:00Z").unwrap();
        assert_eq!(seen, Seen::New);
        assert_eq!(path, dir.join(format!("{}.toml", host.fingerprint())));
        assert_eq!(profile.settings, detected);
        assert_eq!(profile.identity.host, "QEMU Standard PC (Q35 + ICH9, 2009)");
        let current = fs::read_to_string(dir.join("current")).unwrap();
        assert_eq!(current.trim(), host.fingerprint());
        let first = Stored::parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(first.identity.first_seen, "2026-09-09T18:30:00Z");
        assert_eq!(first.set, Layer::default());

        // a person claims the machine and turns the scale up, comment and all
        let edited = format!(
            "{}\n[set]\nclass = \"owned\" # mine\n\n[[set.display]]\nconnector = \"Virtual-1\"\nscale = 2\n",
            fs::read_to_string(&path).unwrap().trim_end()
        );
        fs::write(&path, edited).unwrap();

        let (_, seen, profile) = record(&dir, &host, &detected, "2026-09-10T08:00:00Z").unwrap();
        assert_eq!(seen, Seen::Again);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("class = \"owned\" # mine"), "{text}");
        assert!(text.contains("[[set.display]]"), "{text}");
        let second = Stored::parse(&text).unwrap();
        assert_eq!(second.identity.first_seen, "2026-09-09T18:30:00Z");
        assert_eq!(second.identity.last_seen, "2026-09-10T08:00:00Z");
        assert_eq!(profile.settings.class, "owned");
        assert_eq!(profile.settings.displays[0].scale, 2);
        assert_eq!(profile.settings.ai_tier, "small");
        assert!(!dir.join(format!("{}.tmp", host.fingerprint())).exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_default_that_changes_reaches_the_machine() {
        let dir = temp_dir("rewrite");
        let host = sample_host();

        record(&dir, &host, &no_edid(), "2026-09-09T18:30:00Z").unwrap();
        let path = path(&dir, &host.fingerprint());
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("ai_tier = \"small\"")
        );

        // the same drive in a machine with more memory: the delta follows the machine
        let (_, _, profile) = record(&dir, &host, &laptop(), "2026-09-10T08:00:00Z").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert!(!text.contains("ai_tier ="), "{text}");
        assert!(text.contains("chassis = \"laptop\""), "{text}");
        assert_eq!(profile.settings.displays[0].connector, "eDP-1");
    }

    #[test]
    fn the_effective_profile_spells_everything_out() {
        let profile = Profile {
            identity: identity(),
            settings: laptop(),
        };
        let text = profile.to_toml();
        for key in [
            "fingerprint",
            "host",
            "first_seen",
            "last_seen",
            "class",
            "chassis",
            "gpu_vendor",
            "gpu_path",
            "ai_tier",
            "connector",
        ] {
            assert!(text.contains(key), "{key} missing from {text}");
        }
        assert!(text.contains("class = \"borrowed\""), "{text}");
    }

    #[test]
    fn table_headers() {
        assert_eq!(table_header("[set]"), Some(Section::Set));
        assert_eq!(table_header("[[set.display]]"), Some(Section::SetDisplay));
        assert_eq!(table_header("[detected]"), Some(Section::Other));
        assert_eq!(table_header("[[detected.display]]"), Some(Section::Other));
        assert_eq!(table_header("class = \"owned\""), None);
        assert_eq!(table_header(""), None);
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
        assert_eq!(strip_comment("12 # b"), "12 ");
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
        // the machine label survives a round trip through the file
        let text = render(
            &Identity {
                host: "Acme \"Pro\" \\ 15".to_owned(),
                ..identity()
            },
            &Settings::default(),
            "",
        );
        assert_eq!(
            Stored::parse(&text).unwrap().identity.host,
            "Acme \"Pro\" \\ 15"
        );
    }
}
