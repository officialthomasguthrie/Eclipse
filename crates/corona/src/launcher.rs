//! The app launcher: desktop entries from the XDG data directories, matched by name in the
//! routing module and started here as detached processes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{env, fs};

/// An app from a desktop entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// The Name field.
    pub name: String,
    /// The Exec field split into words, field codes removed.
    pub exec: Vec<String>,
    /// The Terminal field: the app wants a terminal around it.
    pub terminal: bool,
}

/// The terminal that wraps apps with `Terminal=true`.
const TERMINAL: [&str; 2] = ["ghostty", "-e"];

/// Every usable app in the data directories, sorted by name, one per entry id.
#[must_use]
pub fn load() -> Vec<App> {
    let mut apps = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in data_dirs() {
        let Ok(entries) = fs::read_dir(dir.join("applications")) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "desktop") {
                continue;
            }
            let Some(id) = path.file_name().map(std::ffi::OsStr::to_os_string) else {
                continue;
            };
            if !seen.insert(id) {
                continue;
            }
            if let Some(app) = fs::read_to_string(&path).ok().as_deref().and_then(parse) {
                apps.push(app);
            }
        }
    }
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps
}

/// The user's data directory first, then the system ones.
fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match env::var_os("XDG_DATA_HOME") {
        Some(home) => dirs.push(PathBuf::from(home)),
        None => {
            if let Some(home) = env::var_os("HOME") {
                dirs.push(Path::new(&home).join(".local/share"));
            }
        }
    }
    let system = env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(
        system
            .split(':')
            .filter(|d| !d.is_empty())
            .map(PathBuf::from),
    );
    dirs
}

/// Read a desktop entry. `None` for anything that is not an app to show: hidden entries,
/// entries without a command, other types.
#[must_use]
pub fn parse(text: &str) -> Option<App> {
    let mut in_entry = false;
    let mut name = None;
    let mut exec = None;
    let mut terminal = false;
    let mut kind = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Name" => name = Some(value.trim().to_string()),
            "Exec" => exec = Some(value.trim().to_string()),
            "Type" => kind = Some(value.trim().to_string()),
            "Terminal" => terminal = value.trim() == "true",
            "NoDisplay" | "Hidden" if value.trim() == "true" => return None,
            _ => {}
        }
    }
    if kind.as_deref() != Some("Application") {
        return None;
    }
    let exec = split_exec(&exec?);
    if exec.is_empty() {
        return None;
    }
    Some(App {
        name: name?,
        exec,
        terminal,
    })
}

/// Split an Exec value the way the desktop entry spec says: quoted words, backslash escapes
/// inside quotes, and the field codes (`%f`, `%U` and friends) dropped.
#[must_use]
pub fn split_exec(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quoted = false;
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            '\\' if quoted => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            '%' if !quoted => {
                if chars.next() == Some('%') {
                    word.push('%');
                }
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words.retain(|w| !w.is_empty());
    words
}

/// Start the app and let it go.
///
/// # Errors
///
/// When the program cannot be started.
pub fn launch(app: &App) -> Result<(), String> {
    let mut words: Vec<&str> = Vec::new();
    if app.terminal {
        words.extend(TERMINAL);
    }
    words.extend(app.exec.iter().map(String::as_str));
    let (program, args) = words
        .split_first()
        .ok_or_else(|| "Nothing to run".to_string())?;
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|e| format!("Could not start {}: {e}", app.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_entry() {
        let text = "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox %u\nIcon=firefox\n\n[Desktop Action new-window]\nName=New window\nExec=firefox --new-window\n";
        let app = parse(text).unwrap();
        assert_eq!(app.name, "Firefox");
        assert_eq!(app.exec, ["firefox"]);
        assert!(!app.terminal);
    }

    #[test]
    fn skips_what_is_not_an_app() {
        assert!(parse("[Desktop Entry]\nType=Link\nName=Docs\nURL=x\n").is_none());
        assert!(
            parse("[Desktop Entry]\nType=Application\nName=Hidden\nExec=x\nNoDisplay=true\n")
                .is_none()
        );
        assert!(parse("[Desktop Entry]\nType=Application\nName=No command\n").is_none());
    }

    #[test]
    fn terminal_apps_are_marked() {
        let app =
            parse("[Desktop Entry]\nType=Application\nName=Helix\nExec=hx %F\nTerminal=true\n")
                .unwrap();
        assert!(app.terminal);
        assert_eq!(app.exec, ["hx"]);
    }

    #[test]
    fn exec_splitting() {
        assert_eq!(split_exec("firefox %u"), ["firefox"]);
        assert_eq!(
            split_exec("/usr/bin/env FOO=1 app --flag %F"),
            ["/usr/bin/env", "FOO=1", "app", "--flag"]
        );
        assert_eq!(
            split_exec("\"/opt/My App/run\" --name \"a b\" 100%%"),
            ["/opt/My App/run", "--name", "a b", "100%"]
        );
        assert_eq!(
            split_exec("\"quote \\\"inside\\\"\" x"),
            ["quote \"inside\"", "x"]
        );
    }
}
