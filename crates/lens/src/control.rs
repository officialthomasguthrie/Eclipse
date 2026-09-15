//! The field, driven from a terminal. The panel listens on a socket in the session's runtime
//! directory; `lens --type`, `lens --enter` and `lens --escape` write one line to it.
//! The runtime directory belongs to one person, so only that person can type into their field.

// only the panel listens on the socket, and the panel is linux only
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

/// The name of the socket inside the runtime directory.
const SOCKET: &str = "lens.sock";

/// One line of the protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Put these words in the field and show what they match.
    Type(String),
    /// Put them in the field, when there are any, then press Enter.
    Enter(String),
    /// Clear the field, the list and the error line.
    Escape,
}

impl Command {
    /// The line that carries this command.
    #[must_use]
    pub fn line(&self) -> String {
        match self {
            Self::Type(words) => format!("type {words}"),
            Self::Enter(words) => format!("enter {words}"),
            Self::Escape => "escape".to_string(),
        }
    }
}

/// Read one line of the protocol. `None` when it is not one of the three.
#[must_use]
pub fn parse(line: &str) -> Option<Command> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    match verb {
        "type" => Some(Command::Type(rest.to_string())),
        "enter" => Some(Command::Enter(rest.to_string())),
        "escape" => Some(Command::Escape),
        _ => None,
    }
}

/// Where the socket is. `None` when the session has no runtime directory.
#[must_use]
pub fn path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join(SOCKET))
}

/// Send one command to the panel that is running.
///
/// # Errors
///
/// When there is no runtime directory, or no panel is listening on the socket.
pub fn send(command: &Command) -> Result<(), String> {
    let path = path().ok_or("Lens could not find the session runtime directory")?;
    let mut stream = UnixStream::connect(&path)
        .map_err(|e| format!("Could not reach the panel on {}: {e}", path.display()))?;
    writeln!(stream, "{}", command.line()).map_err(|e| format!("Could not write to it: {e}"))
}

/// Listen on the socket and hand every command to `each`. Blocks; the panel calls it on a
/// thread of its own.
///
/// # Errors
///
/// When there is no runtime directory or the socket cannot be opened.
pub fn serve<F: Fn(Command)>(each: F) -> Result<(), String> {
    let path = path().ok_or("Lens could not find the session runtime directory")?;
    // a socket file an earlier run left behind refuses the bind, and no one else owns this name
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)
        .map_err(|e| format!("Could not listen on {}: {e}", path.display()))?;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let mut line = String::new();
        if BufReader::new(stream).read_line(&mut line).is_err() {
            continue;
        }
        if let Some(command) = parse(&line) {
            each(command);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_verbs_read_back() {
        assert_eq!(
            parse("type wifi off\n"),
            Some(Command::Type("wifi off".into()))
        );
        assert_eq!(
            parse("enter ls | first 3"),
            Some(Command::Enter("ls | first 3".into()))
        );
        assert_eq!(parse("escape\r\n"), Some(Command::Escape));
        assert_eq!(parse("enter"), Some(Command::Enter(String::new())));
        assert_eq!(parse("type "), Some(Command::Type(String::new())));
    }

    #[test]
    fn anything_else_is_not_a_command() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("quit"), None);
        assert_eq!(parse("Type wifi"), None);
    }

    #[test]
    fn a_command_survives_the_round_trip() {
        for command in [
            Command::Type("power off".into()),
            Command::Enter("echo hello".into()),
            Command::Escape,
        ] {
            assert_eq!(parse(&command.line()), Some(command));
        }
    }
}
