//! Starting `eclipse-flash write` with the rights to erase a disk, and reading what it prints while it
//! writes. On Linux pkexec asks for the password and the passphrase goes in on stdin. macOS asks with
//! its administrator prompt and Windows with its UAC prompt. Neither passes on what the elevated
//! program prints, so it goes into two files in the temporary folder, which are read as they grow.

#![cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Duration;

use iced::futures::channel::mpsc::UnboundedSender;

/// A write of a drive, the way the command line is asked for it.
#[derive(Debug, Clone)]
pub struct Job {
    /// eclipse-flash.
    pub program: PathBuf,
    /// The image.
    pub image: PathBuf,
    /// The disk: `/dev/sdb`, `/dev/disk4`, `\\.\PhysicalDrive2`.
    pub disk: String,
    /// Its serial, or its name when it has none, as it was typed.
    pub serial: String,
    /// The passphrase for persist on Linux. Without one the drive makes persist when it first starts.
    pub passphrase: Option<String>,
}

/// What the command line did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A line it printed.
    Said(String),
    /// A line it printed about what went wrong, or one about why it did not start.
    Failed(String),
    /// It ended, having written the drive or not.
    Ended(bool),
}

type Events = UnboundedSender<Event>;

fn send(events: &Events, event: Event) {
    let _ = events.unbounded_send(event);
}

impl Job {
    /// The words after `eclipse-flash`.
    fn args(&self) -> Result<Vec<String>, String> {
        let image = self.image.to_str().ok_or_else(|| {
            format!(
                "The name of {} has characters that cannot be passed on. Rename the file.",
                self.image.display()
            )
        })?;
        let mut args = vec!["write".to_string()];
        if self.passphrase.is_none() && cfg!(target_os = "linux") {
            args.push("--first-boot".into());
        }
        args.extend([
            "--serial".to_string(),
            self.serial.clone(),
            image.to_string(),
            self.disk.clone(),
        ]);
        Ok(args)
    }
}

/// Starts the write on a thread of its own. What happens is sent to `events`, which ends with
/// [`Event::Ended`].
pub fn start(job: Job, events: Events) {
    std::thread::spawn(move || {
        let written = run(&job, &events).unwrap_or_else(|why| {
            for line in why.lines() {
                send(&events, Event::Failed(line.to_string()));
            }
            false
        });
        send(&events, Event::Ended(written));
    });
}

/// pkexec's exit code when the password dialog was closed.
#[cfg(target_os = "linux")]
const DISMISSED: i32 = 126;

#[cfg(target_os = "linux")]
fn run(job: &Job, events: &Events) -> Result<bool, String> {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    let mut child = Command::new("pkexec")
        .arg(&job.program)
        .args(job.args()?)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start pkexec, which asks for the rights to write: {e}"))?;
    if let (Some(mut stdin), Some(passphrase)) = (child.stdin.take(), &job.passphrase) {
        let _ = writeln!(stdin, "{passphrase}");
    }
    // closed, so eclipse-flash never waits for a line it will not get
    drop(child.stdin.take());
    let stderr = child.stderr.take();
    let failed = events.clone();
    let errors = std::thread::spawn(move || {
        for line in stderr
            .map(BufReader::new)
            .into_iter()
            .flat_map(BufRead::lines)
            .map_while(Result::ok)
        {
            send(&failed, Event::Failed(line));
        }
    });
    for line in child
        .stdout
        .take()
        .map(BufReader::new)
        .into_iter()
        .flat_map(BufRead::lines)
        .map_while(Result::ok)
    {
        send(events, Event::Said(line));
    }
    let _ = errors.join();
    let status = child.wait().map_err(|e| format!("pkexec stopped: {e}"))?;
    if status.code() == Some(DISMISSED) {
        return Err("Nothing was written, since the password was not given.".into());
    }
    Ok(status.success())
}

#[cfg(target_os = "macos")]
fn run(job: &Job, events: &Events) -> Result<bool, String> {
    use std::process::{Command, Stdio};

    let (out, err) = outputs();
    let mut words = vec![job.program.to_string_lossy().into_owned()];
    words.extend(job.args()?);
    let line = mac_line(&words, (&out, &err));
    let mut child = Command::new("osascript")
        .args(["-e", "on run argv", "-e", MAC_SCRIPT, "-e", "end run"])
        .arg(&line)
        .arg(format!(
            "Writing Eclipse erases {}, and that needs an administrator.",
            job.disk
        ))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!("Could not start osascript, which asks for the rights to write: {e}")
        })?;
    let (status, failed) = follow(&mut child, (&out, &err), events)?;
    let said = told(&mut child);
    if status.success() {
        return Ok(true);
    }
    // what AppleScript gives when the password dialog is cancelled
    if said.contains("(-128)") {
        return Err("Nothing was written, since the administrator password was not given.".into());
    }
    if failed { Ok(false) } else { Err(said) }
}

/// What osascript runs: the shell line in the first argument, with the prompt in the second.
#[cfg(target_os = "macos")]
const MAC_SCRIPT: &str =
    "do shell script (item 1 of argv) with prompt (item 2 of argv) with administrator privileges";

/// The line the administrator's shell runs: every word in single quotes, what it prints sent into the
/// two files.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac_line(words: &[String], (out, err): (&Path, &Path)) -> String {
    let quoted = |word: &str| format!("'{}'", word.replace('\'', r"'\''"));
    let words: Vec<String> = words.iter().map(|word| quoted(word)).collect();
    format!(
        "{} > {} 2> {}",
        words.join(" "),
        quoted(&out.to_string_lossy()),
        quoted(&err.to_string_lossy())
    )
}

/// Start-Process's exit code here when Windows did not start the program, the prompt declined among
/// the reasons. It is ERROR_CANCELLED.
#[cfg(windows)]
const NOT_STARTED: i32 = 1223;

#[cfg(windows)]
fn run(job: &Job, events: &Events) -> Result<bool, String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    /// A console program started by one without a console opens no window.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let (out, err) = outputs();
    let line = windows_line(&job.program, &job.args()?, (&out, &err))?;
    let script = format!(
        "[Console]::OutputEncoding = [Text.Encoding]::UTF8; \
         try {{ $p = Start-Process -FilePath $env:ComSpec -ArgumentList '{}' -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode }} \
         catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit {NOT_STARTED} }}",
        line.replace('\'', "''")
    );
    let mut child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &encoded(&script),
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!("Could not start PowerShell, which asks for the rights to write: {e}")
        })?;
    let (status, failed) = follow(&mut child, (&out, &err), events)?;
    let said = told(&mut child);
    if status.success() {
        return Ok(true);
    }
    if status.code() == Some(NOT_STARTED) {
        return Err(format!("Nothing was written. {said}"));
    }
    if failed { Ok(false) } else { Err(said) }
}

/// What cmd runs as an administrator: eclipse-flash and its words in double quotes, what it prints
/// sent into the two files. cmd cannot quote a double quote or a percent sign, so a word with one is
/// refused.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_line(
    program: &Path,
    args: &[String],
    (out, err): (&Path, &Path),
) -> Result<String, String> {
    let quoted = |word: &str| {
        if word.contains(['"', '%']) {
            Err(format!(
                "{word} has a double quote or a percent sign in it, which Windows cannot pass on. Rename or move the file."
            ))
        } else {
            Ok(format!("\"{word}\""))
        }
    };
    let mut words = vec![quoted(&program.to_string_lossy())?];
    for arg in args {
        words.push(quoted(arg)?);
    }
    Ok(format!(
        "/d /s /c \"{} > {} 2> {}\"",
        words.join(" "),
        quoted(&out.to_string_lossy())?,
        quoted(&err.to_string_lossy())?
    ))
}

/// A PowerShell script the way `-EncodedCommand` takes it: base64 of its UTF-16. No quoting of the
/// script can go wrong on the way.
#[cfg_attr(not(windows), allow(dead_code))]
fn encoded(script: &str) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut text = String::new();
    for group in bytes.chunks(3) {
        let mut number = 0_u32;
        for (index, byte) in group.iter().enumerate() {
            number |= u32::from(*byte) << (16 - 8 * index);
        }
        for index in 0..4 {
            if index <= group.len() {
                let digit = usize::try_from((number >> (18 - 6 * index)) & 63).unwrap_or(0);
                text.push(char::from(DIGITS[digit]));
            } else {
                text.push('=');
            }
        }
    }
    text
}

/// The two files an elevated eclipse-flash prints into, in the temporary folder.
fn outputs() -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("eclipse-flash-{}", std::process::id()));
    let (out, err) = (base.with_extension("out"), base.with_extension("err"));
    // a write before this one left its files, which are read from the start
    let _ = fs::remove_file(&out);
    let _ = fs::remove_file(&err);
    (out, err)
}

/// A file another process prints into, read a line at a time as it grows. It is opened again for
/// each read, so the file is never held open while the other process opens it.
struct Lines {
    path: PathBuf,
    at: u64,
    partial: Vec<u8>,
}

impl Lines {
    fn new(path: &Path) -> Lines {
        Lines {
            path: path.to_path_buf(),
            at: 0,
            partial: Vec::new(),
        }
    }

    /// Hands each whole line that came since the last read to `each`, and with `last` what is left.
    fn read(&mut self, last: bool, each: &mut dyn FnMut(String)) {
        if let Ok(mut file) = File::open(&self.path) {
            let before = self.partial.len();
            if file.seek(SeekFrom::Start(self.at)).is_ok() {
                let _ = file.read_to_end(&mut self.partial);
            }
            self.at += u64::try_from(self.partial.len() - before).unwrap_or(0);
        }
        while let Some(end) = self.partial.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            each(line_of(&line));
        }
        if last && !self.partial.is_empty() {
            let line = std::mem::take(&mut self.partial);
            each(line_of(&line));
        }
    }
}

fn line_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\n', '\r'])
        .to_string()
}

/// Waits for `child`, sending the lines the two files get while it runs, and removes the files after.
/// Returns how it ended and whether anything was printed about what went wrong.
fn follow(
    child: &mut Child,
    (out, err): (&Path, &Path),
    events: &Events,
) -> Result<(std::process::ExitStatus, bool), String> {
    let mut said = Lines::new(out);
    let mut errors = Lines::new(err);
    let mut failed = false;
    let status = loop {
        let status = child
            .try_wait()
            .map_err(|e| format!("Could not wait for the write: {e}"))?;
        let last = status.is_some();
        said.read(last, &mut |line| send(events, Event::Said(line)));
        errors.read(last, &mut |line| {
            failed = true;
            send(events, Event::Failed(line));
        });
        if let Some(status) = status {
            break status;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let _ = fs::remove_file(out);
    let _ = fs::remove_file(err);
    Ok((status, failed))
}

/// What the program that asked for the rights printed on stderr, trimmed.
fn told(child: &mut Child) -> String {
    let mut said = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut said);
    }
    said.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(passphrase: Option<&str>) -> Job {
        Job {
            program: "/opt/eclipse/eclipse-flash".into(),
            image: "/home/thomas/eclipse 0.1.0.raw.zst".into(),
            disk: "/dev/sdb".into(),
            serial: "4C530001".into(),
            passphrase: passphrase.map(str::to_string),
        }
    }

    #[test]
    fn the_command_line_gets_the_serial_and_never_the_passphrase() {
        let args = job(Some("correct horse")).args().unwrap();
        assert_eq!(
            args,
            [
                "write",
                "--serial",
                "4C530001",
                "/home/thomas/eclipse 0.1.0.raw.zst",
                "/dev/sdb"
            ]
        );
        let later = job(None).args().unwrap();
        assert_eq!(
            later.get(1).map(String::as_str) == Some("--first-boot"),
            cfg!(target_os = "linux")
        );
    }

    #[test]
    fn a_mac_shell_gets_every_word_quoted() {
        let words = [
            "/Applications/eclipse-flash".to_string(),
            "write".to_string(),
            "/Users/thomas/Thomas's image.raw".to_string(),
        ];
        assert_eq!(
            mac_line(&words, (Path::new("/tmp/f.out"), Path::new("/tmp/f.err"))),
            r"'/Applications/eclipse-flash' 'write' '/Users/thomas/Thomas'\''s image.raw' > '/tmp/f.out' 2> '/tmp/f.err'"
        );
    }

    #[test]
    fn cmd_gets_every_word_in_double_quotes() {
        let args = vec![
            "write".to_string(),
            r"C:\Users\thomas\eclipse.raw.zst".to_string(),
            r"\\.\PhysicalDrive2".to_string(),
        ];
        assert_eq!(
            windows_line(
                Path::new(r"C:\Eclipse\eclipse-flash.exe"),
                &args,
                (Path::new(r"C:\Temp\f.out"), Path::new(r"C:\Temp\f.err"))
            )
            .unwrap(),
            r#"/d /s /c ""C:\Eclipse\eclipse-flash.exe" "write" "C:\Users\thomas\eclipse.raw.zst" "\\.\PhysicalDrive2" > "C:\Temp\f.out" 2> "C:\Temp\f.err"""#
        );
        let percent = vec![r"C:\100%\eclipse.raw".to_string()];
        assert!(
            windows_line(
                Path::new("eclipse-flash.exe"),
                &percent,
                (Path::new("a"), Path::new("b"))
            )
            .unwrap_err()
            .contains("percent sign")
        );
    }

    #[test]
    fn scripts_are_encoded_as_powershell_reads_them() {
        // what [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes(...)) gives
        assert_eq!(encoded("exit 3"), "ZQB4AGkAdAAgADMA");
        assert_eq!(encoded("ab"), "YQBiAA==");
        assert_eq!(encoded("a"), "YQA=");
    }

    #[test]
    fn a_growing_file_is_read_a_line_at_a_time() {
        let path =
            std::env::temp_dir().join(format!("eclipse-flash-app-lines-{}", std::process::id()));
        fs::write(&path, "Copying the boot partition.\nCopied 1").unwrap();
        let mut lines = Lines::new(&path);
        let mut got = Vec::new();
        lines.read(false, &mut |line| got.push(line));
        assert_eq!(got, ["Copying the boot partition."]);
        fs::write(
            &path,
            "Copying the boot partition.\nCopied 1.0 GiB of 6.3 GiB.\r\nThe end",
        )
        .unwrap();
        lines.read(false, &mut |line| got.push(line));
        lines.read(true, &mut |line| got.push(line));
        assert_eq!(
            got,
            [
                "Copying the boot partition.",
                "Copied 1.0 GiB of 6.3 GiB.",
                "The end"
            ]
        );
        fs::remove_file(&path).unwrap();
    }
}
