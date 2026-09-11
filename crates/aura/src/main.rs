//! aurad: Aura's daemon. It picks a chat model from the manifest for the tier Syzygy reports,
//! runs llama-server on a unix socket as its child, serves the local api on the loopback address in
//! front of it, and answers questions on the system bus as `dev.eclipse.Aura`.

mod api;
mod backend;
mod bus;
mod chat;
mod http;
mod models;

use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use backend::{Backend, Status};
use libeclipse::Component;
use models::{Manifest, Tier};

/// The local api's port. Ollama's, so tools that look for a local model find this one.
const PORT: u16 = 11434;
/// llama-server's socket, in the runtime directory systemd gives aura's user alone.
const SOCKET: &str = "/run/aura/llama.sock";
/// Context size in tokens.
const CTX_SIZE: u32 = 8192;

struct Args {
    manifest: PathBuf,
    models_dir: PathBuf,
    llama_server: PathBuf,
    port: u16,
    socket: PathBuf,
    ctx_size: u32,
    model: Option<String>,
    tier: Option<Tier>,
    print: bool,
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("aurad: {message}");
            usage();
            return ExitCode::from(2);
        }
    };
    let manifest = match Manifest::load(&args.manifest) {
        Ok(manifest) => manifest,
        Err(e) => {
            eprintln!("aurad: {e}");
            return ExitCode::FAILURE;
        }
    };
    let backend = Backend {
        program: args.llama_server,
        models_dir: args.models_dir,
        socket: args.socket,
        ctx_size: args.ctx_size,
    };

    if args.print {
        let on_drive = |file: &str| backend.models_dir.join(file).is_file();
        return match manifest.pick(args.tier, args.model.as_deref(), on_drive) {
            Ok(pick) => {
                println!("{}", pick.reason);
                println!("{}", backend.models_dir.join(&pick.chat.file).display());
                ExitCode::SUCCESS
            }
            Err(why) => {
                println!("{why}");
                ExitCode::FAILURE
            }
        };
    }

    let status = Arc::new(Mutex::new(Status::default()));
    // the local api is up before the model is, so a program gets told the model is loading
    // instead of finding nothing on the port
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, args.port)) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("aurad: could not listen on 127.0.0.1:{}: {e}", args.port);
            return ExitCode::FAILURE;
        }
    };
    {
        let socket = backend.socket.clone();
        let status = Arc::clone(&status);
        thread::spawn(move || api::serve(&listener, &socket, &status));
    }
    let connection = match bus::connect(Arc::clone(&status), backend.socket.clone()) {
        Ok(connection) => connection,
        Err(e) => {
            eprintln!("aurad: could not connect to the system bus: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tier = args.tier.or_else(|| match bus::syzygy_tier(&connection) {
        Ok(word) => {
            let tier = Tier::parse(&word);
            if tier.is_none() {
                println!("aura: syzygy reports tier {word:?}, which aura does not know");
            }
            tier
        }
        Err(e) => {
            println!("aura: could not read the tier from syzygy: {e}");
            None
        }
    });
    tier.map_or("", Tier::name)
        .clone_into(&mut status.lock().unwrap_or_else(PoisonError::into_inner).tier);
    if let Err(e) = bus::take_name(&connection) {
        eprintln!(
            "aurad: could not take the name {}: {e}",
            Component::Aura.dbus_name()
        );
        return ExitCode::FAILURE;
    }

    let notify = || {
        if let Err(e) = bus::announce(&connection) {
            eprintln!("aurad: could not signal a change on the bus: {e}");
        }
    };
    backend.supervise(&manifest, tier, args.model.as_deref(), &status, &notify)
}

/// `Ok(None)` means the program already did what was asked (help or version).
fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut parsed = Args {
        manifest: PathBuf::from(libeclipse::paths::MODEL_MANIFEST),
        models_dir: PathBuf::from(libeclipse::paths::MODELS),
        llama_server: PathBuf::from("llama-server"),
        port: PORT,
        socket: PathBuf::from(SOCKET),
        ctx_size: CTX_SIZE,
        model: None,
        tier: None,
        print: false,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest" => parsed.manifest = value(&mut args, &arg, "a file")?.into(),
            "--models-dir" => parsed.models_dir = value(&mut args, &arg, "a directory")?.into(),
            "--llama-server" => parsed.llama_server = value(&mut args, &arg, "a program")?.into(),
            "--port" => parsed.port = number(&value(&mut args, &arg, "a port")?, &arg)?,
            "--socket" => {
                let path = value(&mut args, &arg, "a file")?;
                if Path::new(&path)
                    .extension()
                    .is_none_or(|extension| extension != "sock")
                {
                    return Err(format!(
                        "--socket needs a file that ends in .sock, llama-server takes anything \
                         else for a host name: {path}"
                    ));
                }
                parsed.socket = path.into();
            }
            "--ctx-size" => parsed.ctx_size = number(&value(&mut args, &arg, "a size")?, &arg)?,
            "--model" => parsed.model = Some(value(&mut args, &arg, "a model id or file")?),
            "--tier" => {
                let word = value(&mut args, &arg, "a tier")?;
                parsed.tier = Some(
                    Tier::parse(&word)
                        .ok_or_else(|| format!("--tier is small, medium or large, not {word}"))?,
                );
            }
            "--print" => parsed.print = true,
            "--version" | "-V" => {
                println!("aurad {}", libeclipse::VERSION);
                return Ok(None);
            }
            "--help" | "-h" => {
                usage();
                return Ok(None);
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(Some(parsed))
}

fn value(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
    what: &str,
) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{flag} needs {what}"))
}

fn number<T: FromStr>(text: &str, flag: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("{flag} needs a number, not {text}"))
}

fn usage() {
    println!("Usage: aurad [options]\n");
    println!("Runs the chat model for this machine and answers on the system bus.\n");
    println!(
        "  --manifest <file>        the model manifest (default {})",
        libeclipse::paths::MODEL_MANIFEST
    );
    println!(
        "  --models-dir <dir>       where the weights are (default {})",
        libeclipse::paths::MODELS
    );
    println!("  --llama-server <program> the inference server (default llama-server)");
    println!("  --port <port>            the local api's port on 127.0.0.1 (default {PORT})");
    println!("  --socket <file>          llama-server's unix socket (default {SOCKET})");
    println!("  --ctx-size <tokens>      its context size (default {CTX_SIZE})");
    println!("  --model <id or file>     run this model instead of the one the tier picks");
    println!("  --tier <tier>            small, medium or large instead of asking syzygy");
    println!("  --print                  print the model that would run and exit");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(list: &[&str]) -> Result<Option<Args>, String> {
        parse_args(list.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn defaults() {
        let args = parse(&[]).unwrap().unwrap();
        assert_eq!(
            args.manifest,
            PathBuf::from(libeclipse::paths::MODEL_MANIFEST)
        );
        assert_eq!(args.models_dir, PathBuf::from(libeclipse::paths::MODELS));
        assert_eq!(args.port, 11434);
        assert_eq!(args.socket, PathBuf::from("/run/aura/llama.sock"));
        assert_eq!(args.ctx_size, 8192);
        assert_eq!(args.model, None);
        assert_eq!(args.tier, None);
        assert!(!args.print);
    }

    #[test]
    fn options() {
        let args = parse(&[
            "--manifest",
            "/tmp/m.toml",
            "--models-dir",
            "/tmp/models",
            "--llama-server",
            "/bin/llama-server",
            "--port",
            "8080",
            "--socket",
            "/tmp/llama.sock",
            "--ctx-size",
            "4096",
            "--model",
            "qwen3-4b-q4_k_m",
            "--tier",
            "medium",
            "--print",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(args.manifest, PathBuf::from("/tmp/m.toml"));
        assert_eq!(args.models_dir, PathBuf::from("/tmp/models"));
        assert_eq!(args.llama_server, PathBuf::from("/bin/llama-server"));
        assert_eq!(args.port, 8080);
        assert_eq!(args.socket, PathBuf::from("/tmp/llama.sock"));
        assert_eq!(args.ctx_size, 4096);
        assert_eq!(args.model.as_deref(), Some("qwen3-4b-q4_k_m"));
        assert_eq!(args.tier, Some(Tier::Medium));
        assert!(args.print);
    }

    #[test]
    fn mistakes() {
        assert!(parse(&["--port"]).is_err());
        assert!(parse(&["--port", "many"]).is_err());
        assert!(parse(&["--socket", "/tmp/llama"]).is_err());
        assert!(parse(&["--tier", "huge"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["--version"]).unwrap().is_none());
    }
}
