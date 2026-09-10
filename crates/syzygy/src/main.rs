//! syzygy: host adaptation. It runs before the session, works out what this machine is, keeps a
//! profile per machine under the hosts directory, and answers on the system bus.

mod bus;
mod displays;
mod edid;
mod gpu;
mod host;
mod profile;
mod sha256;

use std::path::PathBuf;
use std::process::ExitCode;

use host::Host;
use profile::Seen;

struct Args {
    hosts_dir: PathBuf,
    print: bool,
    serve: bool,
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
    let detected = profile::detect();
    let now = profile::now();

    if args.print {
        // what is remembered about this machine, or what would be written for it
        let stored = match profile::load(&args.hosts_dir, &host.fingerprint()) {
            Ok(stored) => stored.unwrap_or_default(),
            Err(e) => {
                eprintln!("syzygy: could not read the stored profile: {e}");
                return ExitCode::FAILURE;
            }
        };
        let mut identity = stored.identity;
        identity.fingerprint = host.fingerprint();
        identity.host = host.label();
        if identity.first_seen.is_empty() {
            identity.first_seen.clone_from(&now);
        }
        identity.last_seen = now;
        print!(
            "{}",
            profile::Profile {
                settings: stored.set.over(detected),
                identity,
            }
            .to_toml()
        );
        return ExitCode::SUCCESS;
    }

    let profile = match profile::record(&args.hosts_dir, &host, &detected, &now) {
        Ok((path, Seen::New, profile)) => {
            println!("syzygy: new machine, wrote {}", path.display());
            profile
        }
        Ok((path, Seen::Again, profile)) => {
            println!("syzygy: known machine, updated {}", path.display());
            profile
        }
        Err(e) => {
            eprintln!(
                "syzygy: could not write the host profile under {}: {e}",
                args.hosts_dir.display()
            );
            return ExitCode::FAILURE;
        }
    };
    println!(
        "syzygy: class {}, {}, gpu {} on {}, ai tier {}, {} output(s)",
        profile.settings.class,
        profile.settings.chassis,
        profile.settings.gpu_path,
        profile.settings.gpu_vendor,
        profile.settings.ai_tier,
        profile.settings.displays.len()
    );

    if !args.serve {
        return ExitCode::SUCCESS;
    }
    if let Err(e) = bus::serve(profile) {
        eprintln!("syzygy: could not answer on the system bus: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// `Ok(None)` means the program already did what was asked (help or version).
fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut hosts_dir = PathBuf::from(libeclipse::paths::HOSTS);
    let mut print = false;
    let mut serve = false;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--hosts-dir" => {
                hosts_dir = PathBuf::from(args.next().ok_or("--hosts-dir needs a directory")?);
            }
            "--print" => print = true,
            "--serve" => serve = true,
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
    Ok(Some(Args {
        hosts_dir,
        print,
        serve,
    }))
}

fn usage() {
    println!("Usage: syzygy [--hosts-dir <dir>] [--print] [--serve]\n");
    println!("Works out what this machine is and writes or updates its profile.\n");
    println!(
        "  --hosts-dir <dir>  where profiles live (default {})",
        libeclipse::paths::HOSTS
    );
    println!("  --print            print the effective profile and write nothing");
    println!("  --serve            after writing, answer on the system bus and stay running");
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
        assert!(!args.serve);
    }

    #[test]
    fn options() {
        let args = parse(&["--hosts-dir", "/tmp/h", "--print", "--serve"])
            .unwrap()
            .unwrap();
        assert_eq!(args.hosts_dir, PathBuf::from("/tmp/h"));
        assert!(args.print);
        assert!(args.serve);
        assert!(parse(&["--hosts-dir"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["--version"]).unwrap().is_none());
    }
}
