//! llama-server as aurad's child. aurad picks the model, starts the server on the loopback
//! address, marks it ready once the model has loaded, and starts it again when it stops. The
//! models directory is looked at again every time, so a model copied onto the drive is picked up
//! without touching the unit.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::http;
use crate::models::{Manifest, Tier};

/// How often the child is looked at.
const POLL: Duration = Duration::from_secs(1);
/// How long a health check may take.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to wait before looking in the models directory again when nothing can run.
const LOOK_AGAIN: Duration = Duration::from_secs(30);
/// The first wait before a stopped server is started again. It doubles up to `LAST_DELAY`.
const FIRST_DELAY: Duration = Duration::from_secs(1);
/// The longest wait before a stopped server is started again.
const LAST_DELAY: Duration = Duration::from_secs(60);
/// A server that ran this long was not failing on start, so the wait starts over.
const STEADY: Duration = Duration::from_secs(60);

/// Where the backend is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No model that fits is on the drive.
    NoModel,
    /// llama-server runs and is loading the model.
    Loading,
    /// The model answers.
    Ready,
    /// llama-server stopped and waits to be started again.
    Failed,
}

impl State {
    /// The word the bus reports.
    pub const fn name(self) -> &'static str {
        match self {
            Self::NoModel => "none",
            Self::Loading => "loading",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

/// What the bus reports about the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// Where the backend is.
    pub state: State,
    /// Manifest id of the model that runs or loads. Empty when there is none.
    pub model: String,
    /// Syzygy's tier. Empty when it did not say.
    pub tier: String,
    /// Why nothing answers, in a sentence. Empty when the model runs.
    pub error: String,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            state: State::NoModel,
            model: String::new(),
            tier: String::new(),
            error: "Aura has not picked a model yet.".into(),
        }
    }
}

/// How to run llama-server.
pub struct Backend {
    /// The llama-server program.
    pub program: PathBuf,
    /// Where the weights are.
    pub models_dir: PathBuf,
    /// The loopback port it listens on.
    pub port: u16,
    /// Context size in tokens.
    pub ctx_size: u32,
}

impl Backend {
    /// llama-server's arguments for one model. It listens on the loopback address only, serves
    /// no web page and never fetches anything.
    pub fn args(&self, model: &Path, alias: &str) -> Vec<String> {
        [
            "--host",
            "127.0.0.1",
            "--port",
            &self.port.to_string(),
            "--model",
            &model.display().to_string(),
            "--alias",
            alias,
            "--ctx-size",
            &self.ctx_size.to_string(),
            "--no-webui",
            "--offline",
        ]
        .iter()
        .map(ToString::to_string)
        .collect()
    }

    /// Runs the backend for as long as aurad runs. `notify` is called after every change to
    /// `status`, with the lock released.
    pub fn supervise(
        &self,
        manifest: &Manifest,
        tier: Option<Tier>,
        named: Option<&str>,
        status: &Mutex<Status>,
        notify: &dyn Fn(),
    ) -> ! {
        let update = |state: State, model: &str, error: &str| {
            let changed = {
                let mut status = status.lock().unwrap_or_else(PoisonError::into_inner);
                let before = status.clone();
                status.state = state;
                model.clone_into(&mut status.model);
                error.clone_into(&mut status.error);
                *status != before
            };
            if changed {
                notify();
            }
        };

        let mut delay = Duration::ZERO;
        let mut said = String::new();
        loop {
            let on_drive = |file: &str| self.models_dir.join(file).is_file();
            let pick = match manifest.pick(tier, named, on_drive) {
                Ok(pick) => pick,
                Err(why) => {
                    if why != said {
                        println!("aura: {why}");
                        said.clone_from(&why);
                    }
                    update(State::NoModel, "", &why);
                    thread::sleep(LOOK_AGAIN);
                    continue;
                }
            };
            if pick.reason != said {
                println!("aura: {}", pick.reason);
                said.clone_from(&pick.reason);
            }

            let model = pick.chat.id.as_str();
            let path = self.models_dir.join(&pick.chat.file);
            update(State::Loading, model, "");
            let started = Instant::now();
            let exit = match Command::new(&self.program)
                .args(self.args(&path, model))
                .spawn()
            {
                Ok(mut child) => {
                    let mut loaded = false;
                    loop {
                        match child.try_wait() {
                            Ok(Some(exit)) => break exit.to_string(),
                            Ok(None) => {}
                            Err(e) => {
                                let _ = child.kill();
                                let _ = child.wait();
                                break e.to_string();
                            }
                        }
                        if !loaded && healthy(self.port) {
                            loaded = true;
                            update(State::Ready, model, "");
                            println!("aura: {model} loaded in {} s", started.elapsed().as_secs());
                        }
                        thread::sleep(POLL);
                    }
                }
                Err(e) => format!("could not start: {e}"),
            };

            delay = next_delay(delay, started.elapsed());
            println!(
                "aura: llama-server stopped ({exit}), starting it again in {} s",
                delay.as_secs()
            );
            update(
                State::Failed,
                model,
                &format!("The model stopped ({exit}). Aura starts it again."),
            );
            thread::sleep(delay);
        }
    }
}

/// True once llama-server has loaded its model.
fn healthy(port: u16) -> bool {
    http::send(port, "GET", "/health", None, HEALTH_TIMEOUT).is_ok_and(|reply| reply.status == 200)
}

/// How long to wait before starting the server again, after it ran for `ran` and the last wait
/// was `previous`.
pub fn next_delay(previous: Duration, ran: Duration) -> Duration {
    if ran >= STEADY {
        FIRST_DELAY
    } else {
        (previous * 2).clamp(FIRST_DELAY, LAST_DELAY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_listens_on_loopback_and_fetches_nothing() {
        let backend = Backend {
            program: PathBuf::from("llama-server"),
            models_dir: PathBuf::from("/var/lib/eclipse/models"),
            port: 11434,
            ctx_size: 8192,
        };
        let args = backend.args(
            Path::new("/var/lib/eclipse/models/Qwen3-0.6B-Q8_0.gguf"),
            "qwen3-0.6b-q8_0",
        );
        let after = |flag: &str| {
            let at = args.iter().position(|arg| arg == flag).unwrap();
            args[at + 1].as_str()
        };
        assert_eq!(after("--host"), "127.0.0.1");
        assert_eq!(after("--port"), "11434");
        assert_eq!(
            after("--model"),
            "/var/lib/eclipse/models/Qwen3-0.6B-Q8_0.gguf"
        );
        assert_eq!(after("--alias"), "qwen3-0.6b-q8_0");
        assert_eq!(after("--ctx-size"), "8192");
        assert!(args.iter().any(|arg| arg == "--no-webui"));
        assert!(args.iter().any(|arg| arg == "--offline"));
    }

    #[test]
    fn a_server_that_keeps_stopping_waits_longer_each_time() {
        let secs = Duration::from_secs;
        assert_eq!(next_delay(Duration::ZERO, secs(0)), secs(1));
        assert_eq!(next_delay(secs(1), secs(3)), secs(2));
        assert_eq!(next_delay(secs(32), secs(3)), secs(60));
        assert_eq!(next_delay(secs(60), secs(3)), secs(60));
        assert_eq!(next_delay(secs(60), secs(600)), secs(1));
    }

    #[test]
    fn states_have_the_words_the_bus_reports() {
        let words: Vec<_> = [State::NoModel, State::Loading, State::Ready, State::Failed]
            .iter()
            .map(|state| state.name())
            .collect();
        assert_eq!(words, ["none", "loading", "ready", "failed"]);
        assert_eq!(Status::default().state, State::NoModel);
    }
}
