//! `rift ai index` and `rift ai search`: search by meaning in home. The index is the owner's
//! own file in their cache folder, which nobody else reads; Quasar only turns text into vectors and
//! never sees a file.

use std::env;
use std::fmt::Write as _;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use librift::quasar::{self, Client};
use librift::search::{self, Hit, Index, Kind};

use crate::text;

const INDEX_USAGE: &str = "Usage: rift ai index";
const INDEX_HELP: &str = "Brings the search index of your home folder up to date. Files that are \
new or changed are read again, and files that are gone are taken out. It also runs by itself \
every 15 minutes.";
const SEARCH_USAGE: &str = "Usage: rift ai search <words>";
const SEARCH_HELP: &str = "Lists the files in your home folder closest in meaning to the words, \
best first, with the line where the closest part starts and the day the file last changed.";

/// How many files a search lists.
const RESULTS: usize = 10;
/// How long an update waits for the embedding model to load.
const LOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// How often it looks while it waits.
const LOOK_EVERY: Duration = Duration::from_secs(2);

pub fn index(args: &[String]) -> ExitCode {
    if let Some(arg) = args.first() {
        if matches!(arg.as_str(), "--help" | "-h") {
            println!("{INDEX_USAGE}\n\n{INDEX_HELP}");
            return ExitCode::SUCCESS;
        }
        return text::unknown("ai index", arg, INDEX_USAGE);
    }
    match update() {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

pub fn search(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            println!("{SEARCH_USAGE}\n\n{SEARCH_HELP}");
            return ExitCode::SUCCESS;
        }
        None => {
            eprintln!("{SEARCH_USAGE}");
            return ExitCode::from(2);
        }
        Some(_) => {}
    }
    match find(&args.join(" ")) {
        Ok(hits) => {
            print!("{}", rows(&hits));
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

fn update() -> Result<(), String> {
    let home = home()?;
    let path = index_path(&home);
    let folder = path.parent().unwrap_or(&home);
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
        .map_err(|e| format!("Could not make {}: {e}", folder.display()))?;
    // one update at a time, so the timer's and one typed in a terminal do not both write
    let _lock = lock(&folder.join("search.lock"))?;
    let model = ready_model()?;
    let client = Client::connect()?;
    // an index that does not read is made again
    let old = fs::read(&path)
        .ok()
        .and_then(|bytes| Index::decode(&bytes).ok());
    let update = search::update(&home, old, &model, &mut |kind, texts| {
        client.embed(kind.name(), texts)
    });
    save(&update.index, &path).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    println!(
        "{}",
        summary(update.index.files.len(), update.read, update.removed)
    );
    update.error.map_or(Ok(()), Err)
}

fn find(words: &str) -> Result<Vec<Hit>, String> {
    let home = home()?;
    let path = index_path(&home);
    let index = match fs::read(&path) {
        Ok(bytes) => Index::decode(&bytes)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err("Nothing is indexed yet. Run rift ai index first.".into());
        }
        Err(e) => return Err(format!("Could not read {}: {e}", path.display())),
    };
    let status = quasar::status()?;
    match status.embedding_state.as_str() {
        "ready" => {}
        "loading" => {
            return Err("The embedding model is still loading. Try again in a moment.".into());
        }
        _ => return Err(status.embedding_error),
    }
    if status.embedding_model != index.model {
        return Err(format!(
            "The index was made with {}, and Quasar runs {}. Run rift ai index to make it again.",
            index.model, status.embedding_model
        ));
    }
    let vectors = Client::connect()?.embed(Kind::Query.name(), &[words.to_string()])?;
    let vector = vectors
        .first()
        .ok_or("Quasar sent no vector for the words.")?;
    let hits = search::rank(&index, &search::normalized(vector), RESULTS);
    if hits.is_empty() {
        return Err("Nothing in your home folder is indexed yet.".into());
    }
    Ok(hits)
}

fn home() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set, so there is no home folder to search.".into())
}

fn index_path(home: &Path) -> PathBuf {
    search::index_path(home, env::var_os("XDG_CACHE_HOME").as_deref())
}

/// The embedding model's id once it is ready. While it loads this waits, up to `LOAD_TIMEOUT`.
fn ready_model() -> Result<String, String> {
    let started = Instant::now();
    loop {
        let status = quasar::status()?;
        match status.embedding_state.as_str() {
            "ready" => return Ok(status.embedding_model),
            "loading" if started.elapsed() < LOAD_TIMEOUT => thread::sleep(LOOK_EVERY),
            "loading" => {
                return Err("The embedding model is still loading. Try again in a moment.".into());
            }
            _ => return Err(status.embedding_error),
        }
    }
}

/// Takes the lock beside the index, and waits while another update holds it. It goes with the
/// file when the process ends.
fn lock(path: &Path) -> Result<fs::File, String> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive)
        .map_err(|e| format!("Could not lock {}: {e}", path.display()))?;
    Ok(file)
}

/// Writes the index whole into a new file and puts it in place of the old one, so a search never
/// reads half an index.
fn save(index: &Index, path: &Path) -> io::Result<()> {
    let new = path.with_extension("new");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&new)?;
    file.write_all(&index.encode())?;
    file.sync_all()?;
    fs::rename(&new, path)
}

fn summary(files: usize, read: usize, removed: usize) -> String {
    let (noun, verb) = if files == 1 {
        ("file", "is")
    } else {
        ("files", "are")
    };
    let was = |count: usize| if count == 1 { "was" } else { "were" };
    format!(
        "{files} {noun} {verb} in the index. {read} {} new or changed, {removed} {} removed.",
        was(read),
        was(removed)
    )
}

/// A row for each file: where in home, with the line, and the day it last changed.
fn rows(hits: &[Hit]) -> String {
    let places: Vec<String> = hits
        .iter()
        .map(|hit| format!("~/{}:{}", hit.path, hit.line))
        .collect();
    let width = places
        .iter()
        .map(|place| place.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for (place, hit) in places.iter().zip(hits) {
        let _ = writeln!(out, "{place:<width$}  {}", search::date(hit.modified));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_counts_in_words() {
        assert_eq!(
            summary(4, 4, 0),
            "4 files are in the index. 4 were new or changed, 0 were removed."
        );
        assert_eq!(
            summary(1, 1, 1),
            "1 file is in the index. 1 was new or changed, 1 was removed."
        );
    }

    #[test]
    fn a_row_is_the_place_and_the_day() {
        let hit = |path: &str, line, modified| Hit {
            path: path.into(),
            line,
            modified,
            score: 0.5,
        };
        assert_eq!(
            rows(&[
                hit("notes/bike.txt", 1, 1_789_221_603_000_000_000),
                hit("code/backup.py", 12, 0),
            ]),
            "~/notes/bike.txt:1   2026-09-12\n~/code/backup.py:12  1970-01-01\n"
        );
    }
}
