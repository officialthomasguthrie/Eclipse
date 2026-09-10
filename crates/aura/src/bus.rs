//! Aura on the system bus: `dev.eclipse.Aura` at `/dev/eclipse/Aura`.
//!
//! `Ask` takes a question and returns the answer as text. It runs nothing. The properties say
//! which model runs, for which tier, and whether it answers yet; every change to them is
//! signalled, so a client can wait for `ready` without polling.

use std::sync::{Arc, Mutex, PoisonError};

use libeclipse::Component;
use zbus::fdo;

use crate::backend::{State, Status};
use crate::chat;

/// The longest question `Ask` takes, in characters.
const LONGEST_QUESTION: usize = 4000;

/// The object that answers on the bus.
pub struct Aura {
    status: Arc<Mutex<Status>>,
    port: u16,
}

impl Aura {
    fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[zbus::interface(name = "dev.eclipse.Aura")]
impl Aura {
    /// Answers a question in plain text.
    async fn ask(&self, question: String) -> fdo::Result<String> {
        let question = admit(&question, &self.status())?;
        let port = self.port;
        blocking::unblock(move || chat::ask(port, &question))
            .await
            .map_err(fdo::Error::Failed)
    }

    /// Manifest id of the model that runs or loads. Empty when there is none.
    #[zbus(property)]
    fn model(&self) -> String {
        self.status().model
    }

    /// The tier Syzygy reported. Empty when it did not say.
    #[zbus(property)]
    fn tier(&self) -> String {
        self.status().tier
    }

    /// `none`, `loading`, `ready` or `failed`.
    #[zbus(property)]
    fn state(&self) -> String {
        self.status().state.name().to_string()
    }

    /// Why nothing answers, in a sentence. Empty when the model runs.
    #[zbus(property)]
    fn error(&self) -> String {
        self.status().error
    }
}

/// The question as it goes to the model, or why it cannot go now.
fn admit(question: &str, status: &Status) -> fdo::Result<String> {
    let question = question.trim();
    if question.is_empty() {
        return Err(fdo::Error::InvalidArgs("Ask needs a question.".into()));
    }
    if question.chars().count() > LONGEST_QUESTION {
        return Err(fdo::Error::InvalidArgs(format!(
            "A question can be at most {LONGEST_QUESTION} characters long."
        )));
    }
    match status.state {
        State::Ready => Ok(question.to_string()),
        State::Loading => Err(fdo::Error::Failed(
            "The model is still loading. Try again in a moment.".into(),
        )),
        State::NoModel | State::Failed => Err(fdo::Error::Failed(status.error.clone())),
    }
}

/// Connects to the system bus and serves the object. The name comes later, from `take_name`.
///
/// # Errors
///
/// When the system bus is not there.
pub fn connect(status: Arc<Mutex<Status>>, port: u16) -> zbus::Result<zbus::blocking::Connection> {
    zbus::blocking::connection::Builder::system()?
        .serve_at(Component::Aura.dbus_path(), Aura { status, port })?
        .build()
}

/// Takes `dev.eclipse.Aura`. systemd counts the service as started from here on.
///
/// # Errors
///
/// When another process owns the name or the policy does not allow it.
pub fn take_name(connection: &zbus::blocking::Connection) -> zbus::Result<()> {
    connection.request_name(Component::Aura.dbus_name())
}

/// Signals every property, after the status changed.
///
/// # Errors
///
/// When the object is not served or the bus is gone.
pub fn announce(connection: &zbus::blocking::Connection) -> zbus::Result<()> {
    let object = connection
        .object_server()
        .interface::<_, Aura>(Component::Aura.dbus_path())?;
    let aura = object.get();
    let emitter = object.signal_emitter();
    zbus::block_on(async {
        aura.state_changed(emitter).await?;
        aura.model_changed(emitter).await?;
        aura.tier_changed(emitter).await?;
        aura.error_changed(emitter).await
    })
}

/// Syzygy's `AiTier`.
///
/// # Errors
///
/// When Syzygy is not on the bus.
pub fn syzygy_tier(connection: &zbus::blocking::Connection) -> zbus::Result<String> {
    let syzygy = Component::Syzygy;
    let proxy: zbus::blocking::Proxy<'_> = zbus::blocking::proxy::Builder::new(connection)
        .destination(syzygy.dbus_name())?
        .path(syzygy.dbus_path())?
        .interface(syzygy.dbus_name())?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()?;
    proxy.get_property("AiTier")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: State, error: &str) -> Status {
        Status {
            state,
            model: "qwen3-0.6b-q8_0".into(),
            tier: "small".into(),
            error: error.into(),
        }
    }

    #[test]
    fn a_question_goes_through_once_the_model_is_ready() {
        let ready = status(State::Ready, "");
        assert_eq!(
            admit("  What is the capital of France?\n", &ready).unwrap(),
            "What is the capital of France?"
        );
    }

    #[test]
    fn an_empty_or_long_question_is_refused() {
        let ready = status(State::Ready, "");
        assert!(matches!(
            admit(" \n", &ready),
            Err(fdo::Error::InvalidArgs(_))
        ));
        let long = "a".repeat(LONGEST_QUESTION + 1);
        assert!(matches!(
            admit(&long, &ready),
            Err(fdo::Error::InvalidArgs(_))
        ));
    }

    #[test]
    fn without_a_ready_model_the_reason_is_the_answer() {
        let loading = status(State::Loading, "");
        assert!(matches!(
            admit("hi", &loading),
            Err(fdo::Error::Failed(why)) if why.contains("still loading")
        ));
        let none = status(
            State::NoModel,
            "No chat model that fits this machine is on the drive.",
        );
        assert!(matches!(
            admit("hi", &none),
            Err(fdo::Error::Failed(why)) if why.starts_with("No chat model")
        ));
    }
}
