//! Vault on the system bus: `dev.eclipse.Vault` at `/dev/eclipse/Vault`.
//!
//! `List` returns the snapshots of home, oldest first. `Take` takes one now and runs the retention
//! rules. `Restore` copies one file back from a snapshot as the account that asked, and refuses
//! with `org.freedesktop.DBus.Error.FileExists` when the file there has changed, unless it is told
//! to replace it.

use std::path::PathBuf;
use std::sync::Arc;

use libeclipse::Component;
use zbus::fdo;
use zbus::message::Header;

use crate::restore::{self, Account, Problem};
use crate::timeline::Timeline;

/// The object that answers on the bus.
pub struct Vault {
    timeline: Arc<Timeline>,
    home: Arc<PathBuf>,
}

#[zbus::interface(name = "dev.eclipse.Vault")]
impl Vault {
    /// Every snapshot of home, oldest first.
    async fn list(&self) -> fdo::Result<Vec<String>> {
        let timeline = Arc::clone(&self.timeline);
        blocking::unblock(move || timeline.list())
            .await
            .map_err(|e| fdo::Error::Failed(format!("Could not read the snapshots: {e}")))
    }

    /// Takes a snapshot of home now and returns its name.
    async fn take(&self) -> fdo::Result<String> {
        let timeline = Arc::clone(&self.timeline);
        let (name, dropped) = blocking::unblock(move || timeline.take())
            .await
            .map_err(fdo::Error::Failed)?;
        println!("vault: took snapshot {name} for the bus");
        for old in dropped {
            println!("vault: dropped snapshot {old}");
        }
        Ok(name)
    }

    /// Restores the file at `path` from `snapshot`: `restored`, `replaced` or `unchanged`, and
    /// the path written.
    #[zbus(out_args("outcome", "path"))]
    async fn restore(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        snapshot: String,
        path: String,
        replace: bool,
    ) -> fdo::Result<(String, String)> {
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::Failed("The request came without a sender.".into()))?
            .to_owned();
        let uid = fdo::DBusProxy::new(connection)
            .await?
            .get_connection_unix_user(sender.into())
            .await?;
        let passwd = std::fs::read_to_string("/etc/passwd")
            .map_err(|e| fdo::Error::Failed(format!("Could not read the password file: {e}")))?;
        let gid = restore::group_of(&passwd, uid).ok_or_else(|| {
            fdo::Error::AccessDenied(format!("Vault does not know the account with uid {uid}."))
        })?;
        let account = Account { uid, gid };

        let timeline = Arc::clone(&self.timeline);
        let home = Arc::clone(&self.home);
        let (outcome, written) = blocking::unblock(move || {
            restore::restore(
                &timeline.snapshots,
                &home,
                &snapshot,
                &path,
                replace,
                account,
            )
        })
        .await
        .map_err(|problem| match problem {
            Problem::Invalid(s) => fdo::Error::InvalidArgs(s),
            Problem::Missing(s) => fdo::Error::FileNotFound(s),
            Problem::Changed(s) => fdo::Error::FileExists(s),
            Problem::Failed(s) => fdo::Error::Failed(s),
        })?;
        println!(
            "vault: {} {} for uid {uid}",
            outcome.name(),
            written.display()
        );
        Ok((outcome.name().to_string(), written.display().to_string()))
    }
}

/// Takes the name and answers until the process is stopped.
///
/// # Errors
///
/// When the system bus is not there, or another process already owns the name.
pub fn serve(timeline: Timeline, home: PathBuf) -> zbus::Result<()> {
    let component = Component::Vault;
    let vault = Vault {
        timeline: Arc::new(timeline),
        home: Arc::new(home),
    };
    let _connection = zbus::blocking::connection::Builder::system()?
        .name(component.dbus_name())?
        .serve_at(component.dbus_path(), vault)?
        .build()?;
    // the connection runs on its own threads; this one has nothing left to do
    loop {
        std::thread::park();
    }
}
