//! Quasar from a client's side. A question goes to quasard on the system bus and comes back as an
//! answer in words or as the words of an OS command. Those words are read with [`os::parse`], the
//! parser a line typed into Lens goes through, so a reply can only propose a command Lens and
//! the rift command already run, and one that changes something is confirmed before it runs.

#[cfg(feature = "bus")]
use std::time::Duration;

use crate::os::{self, Action};
#[cfg(feature = "bus")]
use crate::{Component, bus};

/// How long an answer may take. A small model on a slow machine needs tens of seconds, and quasard
/// gives the model five minutes.
#[cfg(feature = "bus")]
const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);

/// How long the vectors for one call to `Embed` may take. quasard gives the model two minutes.
#[cfg(feature = "bus")]
const EMBED_TIMEOUT: Duration = Duration::from_secs(150);

/// What a reply to `Ask` means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Words to show.
    Answer(String),
    /// One of the OS commands. One that changes something is confirmed first.
    Action(Action),
    /// Nothing to show or run. Holds the sentence that says why.
    Refused(String),
}

/// Quasar's properties: which model runs, for which tier, and whether it answers yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// `none`, `loading`, `ready` or `failed`.
    pub state: String,
    /// Manifest id of the model that runs or loads. Empty when there is none.
    pub model: String,
    /// The tier Orbit reported. Empty when it did not say.
    pub tier: String,
    /// Why nothing answers, in a sentence. Empty when the model runs.
    pub error: String,
    /// The embedding model's state, for search by meaning: `none`, `loading`, `ready` or `failed`.
    pub embedding_state: String,
    /// Manifest id of the embedding model that runs or loads. Empty when there is none.
    pub embedding_model: String,
    /// Why search by meaning cannot run, in a sentence. Empty when the embedding model runs.
    pub embedding_error: String,
}

/// What the two strings `Ask` returned mean.
#[must_use]
pub fn read(kind: &str, text: &str) -> Reply {
    match kind {
        "answer" if text.trim().is_empty() => Reply::Refused("Quasar gave an empty answer.".into()),
        "answer" => Reply::Answer(text.trim().to_string()),
        "action" => {
            let words: Vec<&str> = text.split_whitespace().collect();
            match os::parse(&words) {
                Some(Ok(action)) => Reply::Action(action),
                _ => Reply::Refused(format!(
                    "Quasar suggested \"{}\", which is not a wifi, display, volume or power command.",
                    words.join(" ")
                )),
            }
        }
        _ => Reply::Refused("Quasar sent a reply this program does not understand.".into()),
    }
}

/// Asks quasard and waits for the kind and the text of its reply, which [`read`] makes sense of.
///
/// # Errors
///
/// A sentence when the bus or Quasar is not there, or Quasar could not answer.
#[cfg(feature = "bus")]
pub fn ask(question: &str) -> Result<(String, String), String> {
    let quasar = Component::Quasar;
    let connection = bus::connect(ANSWER_TIMEOUT)?;
    let proxy = bus::proxy(&connection, quasar)?;
    proxy
        .call("Ask", &(question,))
        .map_err(|e| bus::sentence(quasar, e))
}

/// Reads Quasar's properties.
///
/// # Errors
///
/// A sentence when the bus or Quasar is not there, or a property could not be read.
#[cfg(feature = "bus")]
pub fn status() -> Result<Status, String> {
    let quasar = Component::Quasar;
    let connection = bus::connect(bus::PROPERTY_TIMEOUT)?;
    let proxy = bus::proxy(&connection, quasar)?;
    let get = |name: &str| {
        proxy
            .get_property::<String>(name)
            .map_err(|e| bus::sentence(quasar, e))
    };
    Ok(Status {
        state: get("State")?,
        model: get("Model")?,
        tier: get("Tier")?,
        error: get("Error")?,
        embedding_state: get("EmbeddingState")?,
        embedding_model: get("EmbeddingModel")?,
        embedding_error: get("EmbeddingError")?,
    })
}

/// A connection to quasard that stays open for many calls, such as the vectors for every part of
/// every file in home.
#[cfg(feature = "bus")]
pub struct Client {
    proxy: zbus::blocking::Proxy<'static>,
}

#[cfg(feature = "bus")]
impl Client {
    /// Connects to the system bus.
    ///
    /// # Errors
    ///
    /// A sentence when the bus or Quasar is not there.
    pub fn connect() -> Result<Self, String> {
        let connection = bus::connect(EMBED_TIMEOUT)?;
        let proxy = bus::proxy(&connection, Component::Quasar)?;
        Ok(Self { proxy })
    }

    /// A vector for each text, in their order. `kind` is `query` for the words a person searches
    /// with and `document` for the parts of a file.
    ///
    /// # Errors
    ///
    /// A sentence when Quasar is not there or could not make the vectors.
    pub fn embed(&self, kind: &str, texts: &[String]) -> Result<Vec<Vec<f64>>, String> {
        self.proxy
            .call("Embed", &(kind, texts))
            .map_err(|e| bus::sentence(Component::Quasar, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_shown() {
        assert_eq!(
            read("answer", " The capital of France is Paris.\n"),
            Reply::Answer("The capital of France is Paris.".into())
        );
        assert!(matches!(read("answer", " \n"), Reply::Refused(_)));
    }

    #[test]
    fn a_command_that_changes_something_is_one_to_confirm() {
        let Reply::Action(action) = read("action", "volume 40") else {
            panic!("volume 40 was not read as a command");
        };
        assert!(action.mutating);
        assert_eq!(action.program, "wpctl");
        assert_eq!(action.summary, "Set the volume to 40 percent");
        let Reply::Action(off) = read("action", "power  off") else {
            panic!("power off was not read as a command");
        };
        assert!(off.mutating);
        assert_eq!(off.args, ["poweroff"]);
    }

    #[test]
    fn a_command_that_only_reads_runs_at_once() {
        let Reply::Action(action) = read("action", "wifi status") else {
            panic!("wifi status was not read as a command");
        };
        assert!(!action.mutating);
        assert_eq!(action.program, "nmcli");
    }

    #[test]
    fn words_that_are_not_an_os_command_are_refused() {
        for words in [
            "rm -rf /",
            "wifi dance",
            "power nap",
            "systemctl poweroff",
            "",
        ] {
            let Reply::Refused(why) = read("action", words) else {
                panic!("{words:?} was not refused");
            };
            assert!(
                why.contains("not a wifi, display, volume or power command"),
                "{why}"
            );
        }
    }

    #[test]
    fn a_kind_nobody_knows_is_refused() {
        assert_eq!(
            read("shell", "rm -rf /"),
            Reply::Refused("Quasar sent a reply this program does not understand.".into())
        );
    }
}
