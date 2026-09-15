//! Penumbra on the system bus: `dev.rift.Penumbra` at `/dev/rift/Penumbra`.
//!
//! `List` returns every app whose network is off or that runs in a sandbox now. `SetNetwork` turns
//! an app's network off or on, in its sandboxes that run now and in every one it starts later, and
//! keeps the apps that are off in a file. `Starting` is what `penumbra start` asks from inside the
//! scope of a new sandbox before bwrap runs: when the app's network is off, the scope's is cut
//! before the answer goes back. When the service starts it makes its table again from that file and
//! the scopes that run. The table stays when the service stops, so what is off stays off.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

use librift::Component;
use librift::penumbra::name_problem;
use zbus::fdo;
use zbus::message::Header;

use crate::net::{self, Scope};

/// The file in the state folder that holds the apps whose network is off.
pub const OFF_FILE: &str = "network-off";

/// How many times a change to the table is tried. A scope that ends between being found and nft
/// reading its path makes nft refuse the whole script, and the next try does not find it.
const TRIES: usize = 5;

/// What `Starting` says to a process that is not in the scope of a sandbox.
const NOT_A_SANDBOX: &str = "Penumbra starts a sandbox only from the scope rift run --sandbox \
makes for it, and this process is not in one.";

/// The apps that are off, where that is kept, and where the scopes are.
pub struct Switch {
    off: BTreeSet<String>,
    file: PathBuf,
    cgroups: PathBuf,
}

impl Switch {
    /// Reads which apps are off from `file`, when it is there.
    pub fn open(file: PathBuf, cgroups: PathBuf) -> Result<Self, String> {
        let off = match fs::read_to_string(&file) {
            Ok(text) => net::read_off(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeSet::new(),
            Err(error) => return Err(format!("Could not read {}: {error}.", file.display())),
        };
        Ok(Self { off, file, cgroups })
    }

    /// Makes the table again, with the scopes of the apps that are off in its set.
    pub fn make_table(&self) -> Result<(), String> {
        self.apply(true)
    }

    fn apply(&self, whole: bool) -> Result<(), String> {
        let mut refused = String::new();
        for _ in 0..TRIES {
            let scopes = self.scopes()?;
            let cut: Vec<&str> = scopes
                .iter()
                .filter(|scope| self.off.contains(&scope.app))
                .map(|scope| scope.path.as_str())
                .collect();
            let script = if whole {
                net::table(&cut)
            } else {
                net::elements(&cut)
            };
            match nft(&script) {
                Ok(()) => return Ok(()),
                Err(why) => refused = why,
            }
        }
        Err(format!("nft refused the network switch: {refused}"))
    }

    fn scopes(&self) -> Result<Vec<Scope>, String> {
        net::scopes(&self.cgroups).map_err(|error| {
            format!(
                "Could not read the cgroups in {}: {error}.",
                self.cgroups.display()
            )
        })
    }

    fn list(&self) -> Result<Vec<(String, bool, u32)>, String> {
        let mut apps: BTreeMap<String, (bool, u32)> = self
            .off
            .iter()
            .map(|app| (app.clone(), (false, 0)))
            .collect();
        for scope in self.scopes()? {
            apps.entry(scope.app).or_insert((true, 0)).1 += 1;
        }
        Ok(apps
            .into_iter()
            .map(|(app, (network, running))| (app, network, running))
            .collect())
    }

    fn set(&mut self, app: &str, on: bool) -> fdo::Result<u32> {
        if let Some(why) = name_problem(app) {
            return Err(fdo::Error::InvalidArgs(why));
        }
        let before = self.off.clone();
        if on {
            self.off.remove(app);
        } else {
            self.off.insert(app.to_string());
        }
        // the set first, then the file. when either fails, both go back to what they were
        if let Err(why) = self.apply(false).and_then(|()| self.keep()) {
            self.off = before;
            let _ = self.apply(false);
            return Err(fdo::Error::Failed(why));
        }
        let running = self
            .scopes()
            .map_err(fdo::Error::Failed)?
            .iter()
            .filter(|scope| scope.app == app)
            .count();
        Ok(u32::try_from(running).unwrap_or(u32::MAX))
    }

    /// Writes the apps that are off into the file, whole or not at all.
    fn keep(&self) -> Result<(), String> {
        let new = self.file.with_extension("new");
        fs::write(&new, net::write_off(&self.off))
            .and_then(|()| fs::rename(&new, &self.file))
            .map_err(|error| format!("Could not write {}: {error}.", self.file.display()))
    }

    /// The app of the sandbox whose process has this cgroup file, with the scope's network cut
    /// first when the app's is off.
    fn starting(&self, cgroup: &str, uid: u32) -> fdo::Result<(String, bool)> {
        let scope = net::scope_of(cgroup, uid)
            .ok_or_else(|| fdo::Error::AccessDenied(NOT_A_SANDBOX.to_string()))?;
        let network = !self.off.contains(&scope.app);
        if !network {
            self.apply(false).map_err(fdo::Error::Failed)?;
        }
        Ok((scope.app, network))
    }
}

/// Runs nft with a script on its standard input. nft's own message when it refuses.
fn nft(script: &str) -> Result<(), String> {
    let mut child = Command::new("nft")
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not run nft: {error}."))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(script.as_bytes())
            .map_err(|error| format!("Could not give nft its script: {error}."))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("nft did not finish: {error}."))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// The object that answers on the bus.
pub struct Penumbra {
    switch: Mutex<Switch>,
}

impl Penumbra {
    fn switch(&self) -> fdo::Result<MutexGuard<'_, Switch>> {
        self.switch.lock().map_err(|_| {
            fdo::Error::Failed("Penumbra stopped halfway through an earlier change.".to_string())
        })
    }
}

#[zbus::interface(name = "dev.rift.Penumbra")]
impl Penumbra {
    /// Every app whose network is off or that runs in a sandbox now: its name, whether it has the
    /// network, and how many of its sandboxes run.
    fn list(&self) -> fdo::Result<Vec<(String, bool, u32)>> {
        self.switch()?.list().map_err(fdo::Error::Failed)
    }

    /// Turns the network of `app` off or on and returns how many of its sandboxes run now.
    fn set_network(&self, app: &str, on: bool) -> fdo::Result<u32> {
        let running = self.switch()?.set(app, on)?;
        let state = if on { "on" } else { "off" };
        println!("penumbra: the network is {state} for {app}, {running} running");
        Ok(running)
    }

    /// The app of the sandbox the caller starts, and whether it has the network. The caller has to
    /// be in the sandbox's scope; its network is cut before this returns when the app's is off.
    #[zbus(out_args("app", "network"))]
    async fn starting(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> fdo::Result<(String, bool)> {
        let (pid, uid) = caller(&header, connection).await?;
        let cgroup = fs::read_to_string(format!("/proc/{pid}/cgroup")).map_err(|error| {
            fdo::Error::Failed(format!(
                "Could not read the cgroup of process {pid}: {error}."
            ))
        })?;
        let started = self.switch()?.starting(&cgroup, uid);
        match &started {
            Ok((app, true)) => println!("penumbra: a sandbox of {app} starts with the network"),
            Ok((app, false)) => println!("penumbra: a sandbox of {app} starts without the network"),
            Err(error) => println!("penumbra: refused a start from process {pid}: {error}"),
        }
        started
    }
}

/// The process id and account of the connection that sent the message.
async fn caller(header: &Header<'_>, connection: &zbus::Connection) -> fdo::Result<(u32, u32)> {
    let sender = header
        .sender()
        .ok_or_else(|| fdo::Error::Failed("The request came without a sender.".into()))?
        .to_owned();
    let bus = fdo::DBusProxy::new(connection).await?;
    let pid = bus
        .get_connection_unix_process_id(sender.clone().into())
        .await?;
    let uid = bus.get_connection_unix_user(sender.into()).await?;
    Ok((pid, uid))
}

/// Makes the table, takes the name and answers until the process is stopped.
///
/// # Errors
///
/// When nft refuses the table, the system bus is not there, or another process owns the name.
pub fn serve(switch: Switch) -> Result<(), String> {
    switch.make_table()?;
    let off: Vec<&str> = switch.off.iter().map(String::as_str).collect();
    println!(
        "penumbra: made the table, the network is off for: {}",
        off.join(" ")
    );
    let component = Component::Penumbra;
    let penumbra = Penumbra {
        switch: Mutex::new(switch),
    };
    let _connection = zbus::blocking::connection::Builder::system()
        .and_then(|builder| builder.name(component.dbus_name()))
        .and_then(|builder| builder.serve_at(component.dbus_path(), penumbra))
        .and_then(zbus::blocking::connection::Builder::build)
        .map_err(|error| format!("Could not answer on the system bus: {error}"))?;
    // the connection runs on its own threads; this one has nothing left to do
    loop {
        std::thread::park();
    }
}
