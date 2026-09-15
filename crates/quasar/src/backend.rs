//! llama-server as quasard's child, one for the chat model and one for the embedding model. quasard
//! picks the model, starts the server on a unix socket in its own runtime directory, marks it
//! ready once the model has loaded, and starts it again when it stops. Nothing but quasar's user can
//! open the sockets; everything else on the machine goes through the local api in `api` and the
//! bus. The models directory is looked at again every time, so a model copied onto the drive is
//! picked up without touching the unit.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::http;

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

/// What the bus reports about a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// Where the backend is.
    pub state: State,
    /// Manifest id of the model that runs or loads. Empty when there is none.
    pub model: String,
    /// Orbit's tier. Empty when it did not say, and for the embedding model.
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
            error: "Quasar has not picked a model yet.".into(),
        }
    }
}

/// What a server is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The chat model, which answers questions.
    Chat,
    /// The embedding model, which turns text into vectors for search by meaning.
    Embedding,
}

impl Role {
    /// How a sentence names the model.
    const fn model(self) -> &'static str {
        match self {
            Self::Chat => "The model",
            Self::Embedding => "The embedding model",
        }
    }

    /// How the log names the server.
    const fn server(self) -> &'static str {
        match self {
            Self::Chat => "llama-server",
            Self::Embedding => "the embedding server",
        }
    }
}

/// The model a server runs, and a line for the log that says why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    /// Manifest id.
    pub id: String,
    /// File name under the models directory.
    pub file: String,
    /// Why this one.
    pub reason: String,
}

/// Picks the model a server runs, given whether a file is in the models directory.
pub type Pick<'a> = dyn Fn(&dyn Fn(&str) -> bool) -> Result<Picked, String> + 'a;

/// How to run llama-server.
pub struct Backend {
    /// The llama-server program.
    pub program: PathBuf,
    /// Where the weights are.
    pub models_dir: PathBuf,
    /// The unix socket it listens on. llama-server takes a host name that ends in `.sock` as one.
    pub socket: PathBuf,
    /// Context size in tokens.
    pub ctx_size: u32,
    /// Chat or embeddings.
    pub role: Role,
}

impl Backend {
    /// llama-server's arguments for one model. It listens on the unix socket only, serves no web
    /// page and never fetches anything. An embedding server reads each text in one batch, and it
    /// refuses a text longer than the batch, so the batch is as long as the context.
    pub fn args(&self, model: &Path, alias: &str) -> Vec<String> {
        let socket = self.socket.display().to_string();
        let model = model.display().to_string();
        let ctx_size = self.ctx_size.to_string();
        let mut args = vec![
            "--host",
            &socket,
            "--model",
            &model,
            "--alias",
            alias,
            "--ctx-size",
            &ctx_size,
            "--no-webui",
            "--offline",
        ];
        if self.role == Role::Embedding {
            args.extend([
                "--embedding",
                "--batch-size",
                &ctx_size,
                "--ubatch-size",
                &ctx_size,
            ]);
        }
        args.into_iter().map(ToString::to_string).collect()
    }

    /// Runs the backend for as long as quasard runs. `pick` says which model runs, given whether a
    /// file is in the models directory. `notify` is called after every change to `status`, with
    /// the lock released.
    pub fn supervise(&self, pick: &Pick<'_>, status: &Mutex<Status>, notify: &dyn Fn()) -> ! {
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
            let picked = match pick(&on_drive) {
                Ok(picked) => picked,
                Err(why) => {
                    if why != said {
                        println!("quasar: {why}");
                        said.clone_from(&why);
                    }
                    update(State::NoModel, "", &why);
                    thread::sleep(LOOK_AGAIN);
                    continue;
                }
            };
            if picked.reason != said {
                println!("quasar: {}", picked.reason);
                said.clone_from(&picked.reason);
            }

            let model = picked.id.as_str();
            let path = self.models_dir.join(&picked.file);
            update(State::Loading, model, "");
            // a server that stopped leaves its socket behind, and a new one cannot bind over it
            let _ = fs::remove_file(&self.socket);
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
                        if !loaded && healthy(&self.socket) {
                            loaded = true;
                            update(State::Ready, model, "");
                            println!(
                                "quasar: {model} loaded in {} s",
                                started.elapsed().as_secs()
                            );
                        }
                        thread::sleep(POLL);
                    }
                }
                Err(e) => format!("could not start: {e}"),
            };

            delay = next_delay(delay, started.elapsed());
            println!(
                "quasar: {} stopped ({exit}), starting it again in {} s",
                self.role.server(),
                delay.as_secs()
            );
            update(
                State::Failed,
                model,
                &format!(
                    "{} stopped ({exit}). Quasar starts it again.",
                    self.role.model()
                ),
            );
            thread::sleep(delay);
        }
    }
}

/// True once llama-server has loaded its model.
fn healthy(socket: &Path) -> bool {
    http::send(socket, "GET", "/health", None, HEALTH_TIMEOUT)
        .is_ok_and(|reply| reply.status == 200)
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

    fn backend(role: Role, socket: &str, ctx_size: u32) -> Backend {
        Backend {
            program: PathBuf::from("llama-server"),
            models_dir: PathBuf::from("/var/lib/rift/models"),
            socket: PathBuf::from(socket),
            ctx_size,
            role,
        }
    }

    fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        let at = args.iter().position(|arg| arg == flag)?;
        Some(args[at + 1].as_str())
    }

    #[test]
    fn the_server_listens_on_its_socket_and_fetches_nothing() {
        let args = backend(Role::Chat, "/run/quasar/llama.sock", 8192).args(
            Path::new("/var/lib/rift/models/Qwen3-0.6B-Q8_0.gguf"),
            "qwen3-0.6b-q8_0",
        );
        assert_eq!(after(&args, "--host"), Some("/run/quasar/llama.sock"));
        assert!(!args.iter().any(|arg| arg == "--port"));
        assert_eq!(
            after(&args, "--model"),
            Some("/var/lib/rift/models/Qwen3-0.6B-Q8_0.gguf")
        );
        assert_eq!(after(&args, "--alias"), Some("qwen3-0.6b-q8_0"));
        assert_eq!(after(&args, "--ctx-size"), Some("8192"));
        assert!(args.iter().any(|arg| arg == "--no-webui"));
        assert!(args.iter().any(|arg| arg == "--offline"));
        assert!(!args.iter().any(|arg| arg == "--embedding"));
    }

    #[test]
    fn the_embedding_server_reads_a_whole_context_in_one_batch() {
        let args = backend(Role::Embedding, "/run/quasar/embed.sock", 2048).args(
            Path::new("/var/lib/rift/models/nomic-embed-text-v1.5.Q8_0.gguf"),
            "nomic-embed-text-v1.5-q8",
        );
        assert_eq!(after(&args, "--host"), Some("/run/quasar/embed.sock"));
        assert!(args.iter().any(|arg| arg == "--embedding"));
        assert!(args.iter().any(|arg| arg == "--offline"));
        for flag in ["--ctx-size", "--batch-size", "--ubatch-size"] {
            assert_eq!(after(&args, flag), Some("2048"), "{flag}");
        }
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
