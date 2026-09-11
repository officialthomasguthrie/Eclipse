//! `eclipse host`: what Syzygy remembers about this machine, one row per setting. Changing a
//! setting needs a method on Syzygy's side of the bus, which does not exist yet.

use std::process::ExitCode;

use libeclipse::syzygy::{self, Host, Output};

use crate::text;

const USAGE: &str = "Usage: eclipse host";

pub fn run(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        None => {}
        Some("--help" | "-h") => {
            println!("{USAGE}\n\nShows what Syzygy remembers about this machine.");
            return ExitCode::SUCCESS;
        }
        Some(other) => return text::unknown("host", other, USAGE),
    }
    match syzygy::host() {
        Ok(host) => {
            print!("{}", text::table(&rows(&host)));
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

fn rows(host: &Host) -> Vec<(&'static str, String)> {
    let mut rows = vec![
        ("Fingerprint", host.fingerprint.clone()),
        ("Class", host.class.clone()),
    ];
    if host.outputs.is_empty() {
        rows.push(("Display", "none".into()));
    }
    for output in &host.outputs {
        rows.push(("Display", describe(output)));
    }
    rows.push(("GPU path", host.gpu_path.clone()));
    rows.push(("AI tier", host.ai_tier.clone()));
    rows
}

fn describe(output: &Output) -> String {
    let Output {
        connector,
        width,
        height,
        scale,
    } = output;
    if *width == 0 {
        format!("{connector}, no EDID, scale {scale}")
    } else {
        format!("{connector}, {width}x{height}, scale {scale}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(outputs: Vec<Output>) -> Host {
        Host {
            fingerprint: "5297c0f65d6a".repeat(5) + "abcd",
            class: "borrowed".into(),
            outputs,
            gpu_path: "none".into(),
            ai_tier: "small".into(),
        }
    }

    #[test]
    fn a_virtual_machine_reads_as_rows() {
        let qemu = host(vec![Output {
            connector: "Virtual-1".into(),
            width: 1280,
            height: 800,
            scale: 1,
        }]);
        let expected = format!(
            "Fingerprint: {}\nClass:       borrowed\nDisplay:     Virtual-1, 1280x800, scale 1\n\
             GPU path:    none\nAI tier:     small\n",
            qemu.fingerprint
        );
        assert_eq!(text::table(&rows(&qemu)), expected);
    }

    #[test]
    fn every_output_gets_a_row() {
        let laptop = host(vec![
            Output {
                connector: "eDP-1".into(),
                width: 2880,
                height: 1800,
                scale: 2,
            },
            Output {
                connector: "HDMI-A-1".into(),
                width: 0,
                height: 0,
                scale: 1,
            },
        ]);
        let displays: Vec<String> = rows(&laptop)
            .into_iter()
            .filter(|(label, _)| *label == "Display")
            .map(|(_, value)| value)
            .collect();
        assert_eq!(
            displays,
            ["eDP-1, 2880x1800, scale 2", "HDMI-A-1, no EDID, scale 1"]
        );
        let headless = rows(&host(Vec::new()));
        assert!(headless.contains(&("Display", "none".to_string())));
    }
}
