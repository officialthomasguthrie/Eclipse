//! Vault on the system bus: `dev.rift.Vault` at `/dev/rift/Vault`.
//!
//! `List` returns the snapshots of home, oldest first. `Take` takes one now and runs the retention
//! rules. `Restore` copies one file back from a snapshot as the account that asked, and refuses
//! with `org.freedesktop.DBus.Error.FileExists` when the file there has changed, unless it is told
//! to replace it. `Backups`, `Backup` and `RestoreBackup` do the same with the backups on the
//! backup disk.

use std::path::PathBuf;
use std::sync::Arc;

use librift::Component;
use zbus::fdo;
use zbus::message::Header;

use crate::backup::Backups;
use crate::restore::{self, Account, Outcome, Problem};
use crate::timeline::{self, Timeline};

/// The object that answers on the bus.
pub struct Vault {
    timeline: Arc<Timeline>,
    backups: Arc<Backups>,
    home: Arc<PathBuf>,
}

#[zbus::interface(name = "dev.rift.Vault")]
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
        let account = caller(&header, connection).await?;
        let timeline = Arc::clone(&self.timeline);
        let home = Arc::clone(&self.home);
        let done = blocking::unblock(move || {
            restore::restore(
                &timeline.snapshots,
                &home,
                &snapshot,
                &path,
                replace,
                account,
            )
        })
        .await;
        answer(done, account)
    }

    /// Every backup on the backup disk, oldest first: its id and when it was made.
    async fn backups(&self) -> fdo::Result<Vec<(String, String)>> {
        let backups = Arc::clone(&self.backups);
        let listed = blocking::unblock(move || backups.list())
            .await
            .map_err(fdo::Error::Failed)?;
        Ok(listed
            .into_iter()
            .map(|made| (made.id, timeline::name_of(made.time)))
            .collect())
    }

    /// Backs up home now and returns the backup's id and when it was made.
    #[zbus(out_args("id", "time"))]
    async fn backup(&self) -> fdo::Result<(String, String)> {
        let backups = Arc::clone(&self.backups);
        let made = blocking::unblock(move || backups.back_up())
            .await
            .map_err(fdo::Error::Failed)?;
        let time = timeline::name_of(made.time);
        println!("vault: backed up home as {} at {time} for the bus", made.id);
        Ok((made.id, time))
    }

    /// Restores the file at `path` from the backup with the id `backup`, the way `Restore` does
    /// from a snapshot.
    #[zbus(out_args("outcome", "path"))]
    async fn restore_backup(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        backup: String,
        path: String,
        replace: bool,
    ) -> fdo::Result<(String, String)> {
        let account = caller(&header, connection).await?;
        let backups = Arc::clone(&self.backups);
        let home = Arc::clone(&self.home);
        let done =
            blocking::unblock(move || backups.restore(&home, &backup, &path, replace, account))
                .await;
        answer(done, account)
    }
}

/// The account that sent the message: its uid from the bus, its group from the password file.
async fn caller(header: &Header<'_>, connection: &zbus::Connection) -> fdo::Result<Account> {
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
    Ok(Account { uid, gid })
}

/// The reply to a restore, with a problem as the error that says what kind it is.
fn answer(
    done: Result<(Outcome, PathBuf), Problem>,
    account: Account,
) -> fdo::Result<(String, String)> {
    let (outcome, written) = done.map_err(|problem| match problem {
        Problem::Invalid(s) => fdo::Error::InvalidArgs(s),
        Problem::Missing(s) => fdo::Error::FileNotFound(s),
        Problem::Changed(s) => fdo::Error::FileExists(s),
        Problem::Failed(s) => fdo::Error::Failed(s),
    })?;
    println!(
        "vault: {} {} for uid {}",
        outcome.name(),
        written.display(),
        account.uid
    );
    Ok((outcome.name().to_string(), written.display().to_string()))
}

/// Takes the name and answers until the process is stopped.
///
/// # Errors
///
/// When the system bus is not there, or another process already owns the name.
pub fn serve(timeline: Timeline, backups: Backups, home: PathBuf) -> zbus::Result<()> {
    backups.clear();
    let component = Component::Vault;
    let vault = Vault {
        timeline: Arc::new(timeline),
        backups: Arc::new(backups),
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
