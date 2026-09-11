//! logind's Lock signal. `loginctl lock-session` asks logind, logind signals the session's object
//! on the system bus, and this starts the lock screen for it unless one is already up.

use std::env;
use std::process::{Child, Command, ExitCode};

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

const LOGIND: &str = "org.freedesktop.login1";

pub fn run() -> ExitCode {
    match listen() {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("umbra-lock: {why}");
            ExitCode::FAILURE
        }
    }
}

fn listen() -> Result<(), String> {
    let program = env::current_exe().map_err(|e| format!("Could not find this program: {e}"))?;
    let bus = Connection::system().map_err(|e| format!("Could not reach the system bus: {e}"))?;
    // pam_systemd sets the session id and umbra passes it on to what it starts
    let id = env::var("XDG_SESSION_ID").unwrap_or_else(|_| "auto".to_string());
    let manager = Proxy::new(
        &bus,
        LOGIND,
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .map_err(|e| format!("Could not talk to logind: {e}"))?;
    let path: OwnedObjectPath = manager
        .call("GetSession", &(id.as_str(),))
        .map_err(|e| format!("logind does not know session {id}: {e}"))?;
    let session = Proxy::new(&bus, LOGIND, path, "org.freedesktop.login1.Session")
        .map_err(|e| format!("Could not talk to logind about session {id}: {e}"))?;
    let signals = session
        .receive_signal("Lock")
        .map_err(|e| format!("Could not listen for logind's Lock signal: {e}"))?;
    eprintln!("umbra-lock: waiting for logind to lock session {id}");

    let mut screen: Option<Child> = None;
    for _ in signals {
        if screen
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
        {
            continue;
        }
        match Command::new(&program).spawn() {
            Ok(child) => screen = Some(child),
            Err(e) => eprintln!("umbra-lock: could not start the lock screen: {e}"),
        }
    }
    Err("The system bus closed the connection.".to_string())
}
