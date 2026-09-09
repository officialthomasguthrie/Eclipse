//! syzygy: runs before the session. Fingerprints the host and keeps a profile per machine under
//! the hosts directory. The rest of host adaptation (displays, GPU path, AI tier) comes later.

mod host;
mod profile;
mod sha256;

use std::path::PathBuf;
use std::process::ExitCode;

use host::Host;
use profile::{Profile, Seen};

struct Args {
    hosts_dir: PathBuf,
    print: bool,
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("syzygy: {message}");
            usage();
            return ExitCode::from(2);
        }
    };

    let host = Host::detect();
    let now = profile::now();
    if args.print {
        // what is remembered about this machine, or what would be written for it
        let stored = match profile::load(&args.hosts_dir, &host.fingerprint()) {
            Ok(stored) => stored,
            Err(e) => {
                eprintln!("syzygy: could not read the stored profile: {e}");
                return ExitCode::FAILURE;
            }
        };
        print!(
            "{}",
            stored.unwrap_or_else(|| Profile::new(host, &now)).to_toml()
        );
        return ExitCode::SUCCESS;
    }

    match profile::record(&args.hosts_dir, &host, &now) {
        Ok((path, Seen::New)) => {
            println!("syzygy: new machine, wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Ok((path, Seen::Again)) => {
            println!("syzygy: known machine, updated {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!(
                "syzygy: could not write the host profile under {}: {e}",
                args.hosts_dir.display()
            );
            ExitCode::FAILURE
        }
    }
}

/// `Ok(None)` means the program already did what was asked (help or version).
fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut hosts_dir = PathBuf::from(libeclipse::paths::HOSTS);
    let mut print = false;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--hosts-dir" => {
                hosts_dir = PathBuf::from(args.next().ok_or("--hosts-dir needs a directory")?);
            }
            "--print" => print = true,
            "--version" | "-V" => {
                println!("syzygy {}", libeclipse::VERSION);
                return Ok(None);
            }
            "--help" | "-h" => {
                usage();
                return Ok(None);
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(Some(Args { hosts_dir, print }))
}

fn usage() {
    println!("Usage: syzygy [--hosts-dir <dir>] [--print]\n");
    println!("Fingerprints this machine and writes or updates its profile.\n");
    println!(
        "  --hosts-dir <dir>  where profiles live (default {})",
        libeclipse::paths::HOSTS
    );
    println!("  --print            print the profile and exit without writing anything");
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
        assert_eq!(args.hosts_dir, PathBuf::from(libeclipse::paths::HOSTS));
        assert!(!args.print);
    }

    #[test]
    fn options() {
        let args = parse(&["--hosts-dir", "/tmp/h", "--print"])
            .unwrap()
            .unwrap();
        assert_eq!(args.hosts_dir, PathBuf::from("/tmp/h"));
        assert!(args.print);
        assert!(parse(&["--hosts-dir"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["--version"]).unwrap().is_none());
    }
}
