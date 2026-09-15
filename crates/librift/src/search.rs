//! Search by meaning, from the owner's side. An update walks home, cuts each text file into parts,
//! gets a vector for each part from Quasar and keeps them all in one index; a search gets a vector
//! for the words and ranks files by their closest part. Quasar turns text into vectors and never
//! reads home, whoever runs this does.
//!
//! The index is one flat file, little endian: [`HEADER`], the model's id, the number of dimensions,
//! then for each file its path relative to home, its modified time and size, and for each of its
//! parts the line the part starts on and the normalized vector.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Extensions of the files that are indexed, in lower case: plain text, markdown and code.
pub const EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "org", "rst", "tex", "csv", "rs", "py", "js", "ts", "go", "c", "h",
    "cpp", "hpp", "java", "kt", "swift", "rb", "php", "lua", "sh", "fish", "nu", "zig", "nix",
    "toml", "yaml", "yml", "json", "html", "css",
];
/// Folders the walk does not go into, besides hidden ones. Builds and package managers fill them.
pub const SKIPPED: &[&str] = &["node_modules", "target", "__pycache__"];
/// The largest file that is indexed, in bytes.
pub const LARGEST_FILE: u64 = 1 << 20;
/// The longest part, in characters. Even at a token a character it fits the model's context.
pub const LONGEST_PART: usize = 1500;
/// A part this long ends at the next blank line.
pub const PARAGRAPH: usize = 500;
/// How many parts go to Quasar in one call.
pub const PARTS_PER_CALL: usize = 16;
/// The first bytes of an index.
pub const HEADER: &[u8] = b"rift search index 1\n";

/// Which side of a search a text is on. The model is told, it reads the two a little differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The words a person searches with.
    Query,
    /// A part of a file that is searched.
    Document,
}

impl Kind {
    /// The word `Embed` takes.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Document => "document",
        }
    }
}

/// A file the walk found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The path relative to home, with `/` between names.
    pub path: String,
    /// When it was last modified, in nanoseconds since 1970.
    pub modified: i64,
    /// Its size in bytes.
    pub size: u64,
}

/// True when a file with this name is indexed.
#[must_use]
pub fn indexable(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

/// The files in `home` that are indexed, by path. Hidden files and folders, the folders in
/// [`SKIPPED`], symbolic links, names that are not UTF-8 and files over [`LARGEST_FILE`] are left
/// out, and so is what cannot be read.
#[must_use]
pub fn walk(home: &Path) -> Vec<Found> {
    let mut found = Vec::new();
    let mut folders = vec![String::new()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = fs::read_dir(home.join(&folder)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str().filter(|name| !name.starts_with('.')) else {
                continue;
            };
            let path = if folder.is_empty() {
                name.to_string()
            } else {
                format!("{folder}/{name}")
            };
            // the type of the entry itself, a link is not followed
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if !SKIPPED.contains(&name) {
                    folders.push(path);
                }
            } else if kind.is_file() && indexable(name) {
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                if metadata.len() <= LARGEST_FILE {
                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                        .and_then(|since| i64::try_from(since.as_nanos()).ok())
                        .unwrap_or(0);
                    found.push(Found {
                        path,
                        modified,
                        size: metadata.len(),
                    });
                }
            }
        }
    }
    found.sort_by(|one, other| one.path.cmp(&other.path));
    found
}

/// A part of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// The line it starts on, counted from 1.
    pub line: u32,
    /// Its text.
    pub text: String,
}

/// A text cut into parts of at most [`LONGEST_PART`] characters. Lines are kept whole, a part ends
/// at a blank line once it has [`PARAGRAPH`] characters, and a line longer than a part is cut by
/// characters. Parts with nothing but white space are left out.
#[must_use]
pub fn parts(text: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut length = 0;
    let mut start = 1;
    let end = |parts: &mut Vec<Part>, current: &mut String, length: &mut usize, start| {
        // blank lines at the end of a part go with it into nothing
        if !current.trim().is_empty() {
            parts.push(Part {
                line: start,
                text: current.trim_end().to_string(),
            });
        }
        current.clear();
        *length = 0;
    };
    for (number, line) in (1..).zip(text.lines()) {
        let blank = line.trim().is_empty();
        if blank && length >= PARAGRAPH {
            end(&mut parts, &mut current, &mut length, start);
            continue;
        }
        let characters = line.chars().count();
        if length > 0 && length + 1 + characters > LONGEST_PART {
            end(&mut parts, &mut current, &mut length, start);
        }
        if characters > LONGEST_PART {
            let characters: Vec<char> = line.chars().collect();
            for piece in characters.chunks(LONGEST_PART) {
                let piece: String = piece.iter().collect();
                if !piece.trim().is_empty() {
                    parts.push(Part {
                        line: number,
                        text: piece,
                    });
                }
            }
            continue;
        }
        if length == 0 {
            if blank {
                continue;
            }
            start = number;
        } else {
            current.push('\n');
            length += 1;
        }
        current.push_str(line);
        length += characters;
    }
    end(&mut parts, &mut current, &mut length, start);
    parts
}

/// The index of one home.
#[derive(Debug, Clone, PartialEq)]
pub struct Index {
    /// Manifest id of the model that made the vectors. Vectors of two models do not compare.
    pub model: String,
    /// The files, by path.
    pub files: Vec<File>,
}

/// A file in the index.
#[derive(Debug, Clone, PartialEq)]
pub struct File {
    /// The path relative to home.
    pub path: String,
    /// When it was last modified, in nanoseconds since 1970, as it was when it was read.
    pub modified: i64,
    /// Its size in bytes, as it was when it was read.
    pub size: u64,
    /// Its parts. None when the file is empty or not text.
    pub parts: Vec<Stored>,
}

/// A part of a file in the index.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    /// The line it starts on, counted from 1.
    pub line: u32,
    /// Its vector, normalized.
    pub vector: Vec<f32>,
}

const DAMAGED: &str = "The search index is damaged.";

impl Index {
    /// How many dimensions the vectors have. 0 when there are none.
    #[must_use]
    pub fn dimensions(&self) -> usize {
        self.files
            .iter()
            .flat_map(|file| &file.parts)
            .map(|part| part.vector.len())
            .next()
            .unwrap_or(0)
    }

    /// The index as it is written to its file. A vector of another length than the first is cut
    /// or filled with zeros, which only happens when a model sent vectors of two lengths.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let dimensions = self.dimensions();
        let mut bytes = HEADER.to_vec();
        let text = |bytes: &mut Vec<u8>, text: &str| {
            bytes.extend(u32::try_from(text.len()).unwrap_or(u32::MAX).to_le_bytes());
            bytes.extend(text.as_bytes());
        };
        let count = |length: usize| u32::try_from(length).unwrap_or(u32::MAX).to_le_bytes();
        text(&mut bytes, &self.model);
        bytes.extend(count(dimensions));
        bytes.extend(count(self.files.len()));
        for file in &self.files {
            text(&mut bytes, &file.path);
            bytes.extend(file.modified.to_le_bytes());
            bytes.extend(file.size.to_le_bytes());
            bytes.extend(count(file.parts.len()));
            for part in &file.parts {
                bytes.extend(part.line.to_le_bytes());
                for at in 0..dimensions {
                    let value = part.vector.get(at).copied().unwrap_or(0.0);
                    bytes.extend(value.to_le_bytes());
                }
            }
        }
        bytes
    }

    /// Reads an index out of its file's bytes.
    ///
    /// # Errors
    ///
    /// A sentence when the bytes are not a whole index.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let mut reader = Reader {
            bytes: bytes.strip_prefix(HEADER).ok_or(DAMAGED)?,
        };
        let model = reader.text()?;
        let dimensions = reader.count()?;
        let files = reader.count()?;
        let mut index = Self {
            model,
            files: Vec::with_capacity(files.min(reader.bytes.len())),
        };
        for _ in 0..files {
            let path = reader.text()?;
            let modified = i64::from_le_bytes(reader.array()?);
            let size = u64::from_le_bytes(reader.array()?);
            let count = reader.count()?;
            let part_bytes = dimensions
                .checked_mul(4)
                .and_then(|bytes| bytes.checked_add(4));
            if part_bytes
                .and_then(|bytes| bytes.checked_mul(count))
                .is_none_or(|bytes| bytes > reader.bytes.len())
            {
                return Err(DAMAGED.into());
            }
            let mut parts = Vec::with_capacity(count);
            for _ in 0..count {
                let line = u32::from_le_bytes(reader.array()?);
                let vector = (0..dimensions)
                    .map(|_| reader.array().map(f32::from_le_bytes))
                    .collect::<Result<_, _>>()?;
                parts.push(Stored { line, vector });
            }
            index.files.push(File {
                path,
                modified,
                size,
                parts,
            });
        }
        if reader.bytes.is_empty() {
            Ok(index)
        } else {
            Err(DAMAGED.into())
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl Reader<'_> {
    fn array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let (head, rest) = self.bytes.split_first_chunk().ok_or(DAMAGED)?;
        self.bytes = rest;
        Ok(*head)
    }

    fn count(&mut self) -> Result<usize, String> {
        usize::try_from(u32::from_le_bytes(self.array()?)).map_err(|_| DAMAGED.to_string())
    }

    fn text(&mut self) -> Result<String, String> {
        let length = self.count()?;
        if length > self.bytes.len() {
            return Err(DAMAGED.into());
        }
        let (text, rest) = self.bytes.split_at(length);
        self.bytes = rest;
        String::from_utf8(text.to_vec()).map_err(|_| DAMAGED.to_string())
    }
}

/// What an update did.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    /// The index as it is now. It is worth saving even when `error` is set.
    pub index: Index,
    /// How many files were new or changed and were read.
    pub read: usize,
    /// How many files of the old index are no longer in home.
    pub removed: usize,
    /// Why the update stopped before it read every new or changed file. The index then has the
    /// files that were read and the ones that did not change.
    pub error: Option<String>,
}

/// Gets vectors for texts, one for each in their order: `Embed` on the bus, or something that
/// stands in for it.
pub type Embedder<'a> = dyn FnMut(Kind, &[String]) -> Result<Vec<Vec<f64>>, String> + 'a;

/// Brings the index of `home` up to date for `model`. A file whose size and modified time did not
/// change keeps its parts; an index of another model starts over.
pub fn update(home: &Path, old: Option<Index>, model: &str, embed: &mut Embedder<'_>) -> Update {
    let mut kept: HashMap<String, File> = old
        .filter(|old| old.model == model)
        .map(|old| {
            old.files
                .into_iter()
                .map(|file| (file.path.clone(), file))
                .collect()
        })
        .unwrap_or_default();
    let mut files = Vec::new();
    let mut read = 0;
    let mut error = None;
    for found in walk(home) {
        let before = kept.remove(&found.path);
        if let Some(before) =
            before.filter(|before| before.modified == found.modified && before.size == found.size)
        {
            files.push(before);
            continue;
        }
        if error.is_some() {
            continue;
        }
        match read_file(&home.join(&found.path), found, embed) {
            Ok(file) => {
                files.push(file);
                read += 1;
            }
            Err(why) => error = Some(why),
        }
    }
    Update {
        index: Index {
            model: model.to_string(),
            files,
        },
        read,
        removed: kept.len(),
        error,
    }
}

/// Reads a file, cuts it into parts and gets their vectors. A file that cannot be read, is empty
/// or is not UTF-8 text gets no parts, so it is not read again until it changes.
fn read_file(full: &Path, found: Found, embed: &mut Embedder<'_>) -> Result<File, String> {
    let text = fs::read(full)
        .ok()
        .filter(|bytes| !bytes.contains(&0))
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default();
    let name = found.path.rsplit('/').next().unwrap_or_default();
    let mut stored = Vec::new();
    for batch in parts(&text).chunks(PARTS_PER_CALL) {
        // the file's name goes with every part, so a part is found by the name as well
        let texts: Vec<String> = batch
            .iter()
            .map(|part| format!("{name}\n{}", part.text))
            .collect();
        let vectors = embed(Kind::Document, &texts)?;
        if vectors.len() != batch.len() {
            return Err(format!(
                "Quasar sent {} vectors for {} texts.",
                vectors.len(),
                batch.len()
            ));
        }
        stored.extend(batch.iter().zip(vectors).map(|(part, vector)| Stored {
            line: part.line,
            vector: normalized(&vector),
        }));
    }
    Ok(File {
        path: found.path,
        modified: found.modified,
        size: found.size,
        parts: stored,
    })
}

/// A vector of length 1, so the dot product of two is their cosine.
#[must_use]
pub fn normalized(vector: &[f64]) -> Vec<f32> {
    let length = vector.iter().map(|value| value * value).sum::<f64>().sqrt();
    let scale = if length > 0.0 { 1.0 / length } else { 0.0 };
    #[allow(clippy::cast_possible_truncation)]
    vector.iter().map(|value| (value * scale) as f32).collect()
}

/// A file a search found.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// The path relative to home.
    pub path: String,
    /// The line its closest part starts on.
    pub line: u32,
    /// When it was last modified, in nanoseconds since 1970.
    pub modified: i64,
    /// How close its closest part is, the cosine of the two vectors.
    pub score: f32,
}

/// The files of the index closest to `query`, a normalized vector, closest first. A file is as
/// close as its closest part, and files without parts are not found.
#[must_use]
pub fn rank(index: &Index, query: &[f32], limit: usize) -> Vec<Hit> {
    let mut hits: Vec<Hit> = index
        .files
        .iter()
        .filter_map(|file| {
            file.parts
                .iter()
                .map(|part| {
                    let score = part
                        .vector
                        .iter()
                        .zip(query)
                        .map(|(one, other)| one * other);
                    (score.sum::<f32>(), part.line)
                })
                .max_by(|one, other| one.0.total_cmp(&other.0))
                .map(|(score, line)| Hit {
                    path: file.path.clone(),
                    line,
                    modified: file.modified,
                    score,
                })
        })
        .collect();
    hits.sort_by(|one, other| {
        other
            .score
            .total_cmp(&one.score)
            .then_with(|| one.path.cmp(&other.path))
    });
    hits.truncate(limit);
    hits
}

/// Where the index of a home is kept: in `rift` under the owner's cache folder, which is
/// `$XDG_CACHE_HOME` when it is set and `.cache` in home when it is not.
#[must_use]
pub fn index_path(home: &Path, cache: Option<&OsStr>) -> PathBuf {
    let cache = cache
        .filter(|cache| Path::new(cache).is_absolute())
        .map_or_else(|| home.join(".cache"), PathBuf::from);
    cache.join("rift").join("search.index")
}

/// The day a time falls on in UTC, as `2026-09-14`. `nanoseconds` count from 1970.
#[must_use]
pub fn date(nanoseconds: i64) -> String {
    let days = nanoseconds.div_euclid(1_000_000_000).div_euclid(86_400);
    // Howard Hinnant's civil_from_days, with eras of 400 years that start on the first of March
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn text_and_code_are_indexed_by_extension() {
        for name in ["notes.md", "bike.TXT", "backup.py", "flake.nix", "main.rs"] {
            assert!(indexable(name), "{name}");
        }
        for name in ["photo.jpg", "taxes.pdf", "README", "archive.tar.gz", ".md"] {
            assert!(!indexable(name), "{name}");
        }
    }

    #[test]
    fn short_lines_make_one_part_that_starts_at_the_first_words() {
        assert_eq!(
            parts("\n\nTomatoes want sun.\nWater the beans.\n\n"),
            [Part {
                line: 3,
                text: "Tomatoes want sun.\nWater the beans.".into()
            }]
        );
        assert!(parts("").is_empty());
        assert!(parts(" \n\t\n").is_empty());
    }

    #[test]
    fn a_long_text_ends_its_parts_at_blank_lines_and_never_goes_over() {
        let paragraph = "word ".repeat(30).trim_end().to_string();
        let mut text = String::new();
        for _ in 0..40 {
            text.push_str(&paragraph);
            text.push_str("\n\n");
        }
        let parts = parts(&text);
        assert!(parts.len() > 1);
        for part in &parts {
            let length = part.text.chars().count();
            assert!(length <= LONGEST_PART, "{length}");
            assert!(length >= PARAGRAPH, "{length}");
            assert!(!part.text.starts_with('\n') && !part.text.ends_with('\n'));
        }
        assert_eq!(parts[0].line, 1);
        // four paragraphs of 149 characters and the blank lines between them pass 500, so every
        // part has four and starts on a paragraph's line
        assert_eq!(parts[1].line, 9);
    }

    #[test]
    fn a_line_longer_than_a_part_is_cut_by_characters() {
        let text = format!("first\n{}\nlast", "\u{e9}".repeat(LONGEST_PART * 2 + 10));
        let parts = parts(&text);
        let lines: Vec<u32> = parts.iter().map(|part| part.line).collect();
        assert_eq!(lines, [1, 2, 2, 2, 3]);
        assert_eq!(parts[1].text.chars().count(), LONGEST_PART);
        assert_eq!(parts[3].text.chars().count(), 10);
    }

    struct Home(PathBuf);

    impl Home {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("rift-search-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, path: &str, contents: &[u8]) {
            let full = self.0.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, contents).unwrap();
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_walk_finds_text_files_and_leaves_the_rest_out() {
        let home = Home::new("walk");
        home.write("notes/garden.md", b"Tomatoes");
        home.write("bike.txt", b"Oil the chain");
        home.write("photo.jpg", b"\xff\xd8");
        home.write(".config/zed/settings.json", b"{}");
        home.write("code/app/node_modules/left/index.js", b"module");
        home.write("code/app/target/debug/build.rs", b"fn main() {}");
        home.write("code/app/src/main.rs", b"fn main() {}");
        home.write(
            "big.txt",
            &vec![b'a'; usize::try_from(LARGEST_FILE).unwrap() + 1],
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink(home.0.join("bike.txt"), home.0.join("link.txt")).unwrap();
        let paths: Vec<String> = walk(&home.0).into_iter().map(|found| found.path).collect();
        assert_eq!(
            paths,
            ["bike.txt", "code/app/src/main.rs", "notes/garden.md"]
        );
        let found = walk(&home.0);
        assert_eq!(found[0].size, 13);
        assert!(found[0].modified > 0);
        assert!(walk(&home.0.join("nothing here")).is_empty());
    }

    /// A stand-in for the model: one dimension for each of a few words, counted.
    fn words(texts: &[String]) -> Vec<Vec<f64>> {
        texts
            .iter()
            .map(|text| {
                ["garden", "bike", "tax", "soup"]
                    .iter()
                    .map(|word| {
                        f64::from(u32::try_from(text.matches(word).count()).unwrap()) + 0.01
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn an_update_reads_only_what_changed_and_a_search_ranks_by_the_closest_part() {
        let home = Home::new("update");
        home.write("garden.md", b"garden garden");
        home.write("bike.txt", b"bike");
        home.write("photo.txt", b"\x00\x01");
        let calls = RefCell::new(Vec::new());
        let mut embed = |kind: Kind, texts: &[String]| {
            calls.borrow_mut().push((kind, texts.to_vec()));
            Ok(words(texts))
        };

        let first = update(&home.0, None, "model", &mut embed);
        assert_eq!(
            (first.read, first.removed, first.error.as_deref()),
            (3, 0, None)
        );
        assert_eq!(first.index.files.len(), 3);
        assert!(
            first
                .index
                .files
                .iter()
                .any(|file| file.path == "photo.txt" && file.parts.is_empty())
        );
        assert_eq!(
            calls.borrow()[0],
            (Kind::Document, vec!["bike.txt\nbike".to_string()])
        );

        calls.borrow_mut().clear();
        home.write("soup.txt", b"soup and more soup");
        fs::remove_file(home.0.join("bike.txt")).unwrap();
        let second = update(&home.0, Some(first.index.clone()), "model", &mut embed);
        assert_eq!((second.read, second.removed), (1, 1));
        assert_eq!(calls.borrow().len(), 1);
        let paths: Vec<&str> = second
            .index
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(paths, ["garden.md", "photo.txt", "soup.txt"]);

        let query = normalized(&words(&["soup".to_string()])[0]);
        let hits = rank(&second.index, &query, 10);
        let found: Vec<&str> = hits.iter().map(|hit| hit.path.as_str()).collect();
        assert_eq!(found, ["soup.txt", "garden.md"]);
        assert_eq!(hits[0].line, 1);
        assert!(hits[0].score > hits[1].score);
        assert_eq!(rank(&second.index, &query, 1).len(), 1);

        // another model starts over
        let other = update(&home.0, Some(second.index), "other", &mut embed);
        assert_eq!((other.read, other.removed), (3, 0));
        assert_eq!(other.index.model, "other");
    }

    #[test]
    fn a_failed_update_keeps_what_did_not_change() {
        let home = Home::new("failed");
        home.write("a.txt", b"garden");
        home.write("b.txt", b"bike");
        let mut working = |_: Kind, texts: &[String]| Ok(words(texts));
        let first = update(&home.0, None, "model", &mut working);
        home.write("b.txt", b"bike bike");
        home.write("c.txt", b"soup");
        let mut broken = |_: Kind, _: &[String]| Err("Quasar is not running.".to_string());
        let second = update(&home.0, Some(first.index), "model", &mut broken);
        assert_eq!(second.error.as_deref(), Some("Quasar is not running."));
        assert_eq!(second.read, 0);
        let paths: Vec<&str> = second
            .index
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(paths, ["a.txt"]);
    }

    #[test]
    fn an_index_reads_back_as_it_was_written() {
        let index = Index {
            model: "nomic-embed-text-v1.5-q8".into(),
            files: vec![
                File {
                    path: "notes/garden.md".into(),
                    modified: 1_789_221_603_123_456_789,
                    size: 42,
                    parts: vec![
                        Stored {
                            line: 1,
                            vector: vec![0.6, 0.8, 0.0],
                        },
                        Stored {
                            line: 9,
                            vector: vec![0.0, -1.0, 0.0],
                        },
                    ],
                },
                File {
                    path: "photo.txt".into(),
                    modified: -5,
                    size: 2,
                    parts: Vec::new(),
                },
            ],
        };
        let bytes = index.encode();
        assert!(bytes.starts_with(HEADER));
        assert_eq!(Index::decode(&bytes).unwrap(), index);
        assert_eq!(
            Index::decode(
                &Index {
                    model: "m".into(),
                    files: Vec::new()
                }
                .encode()
            )
            .unwrap()
            .dimensions(),
            0
        );
        for cut in [0, HEADER.len(), HEADER.len() + 3, bytes.len() - 1] {
            assert!(Index::decode(&bytes[..cut]).is_err(), "{cut}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(Index::decode(&longer).is_err());
        // a part count that would not fit in the file is refused before anything is allocated
        let mut huge = Index {
            model: "m".into(),
            files: vec![File {
                path: "a".into(),
                modified: 0,
                size: 0,
                parts: Vec::new(),
            }],
        }
        .encode();
        let at = huge.len() - 4;
        huge[at..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Index::decode(&huge).is_err());
    }

    #[test]
    fn the_index_is_in_the_owners_cache() {
        let home = Path::new("/home/rift");
        // an absolute path on every system, /tmp is not one on windows
        let cache = std::env::temp_dir().join("cache");
        assert_eq!(
            index_path(home, None),
            Path::new("/home/rift/.cache/rift/search.index")
        );
        assert_eq!(
            index_path(home, Some(cache.as_os_str())),
            cache.join("rift").join("search.index")
        );
        assert_eq!(
            index_path(home, Some(OsStr::new("relative"))),
            Path::new("/home/rift/.cache/rift/search.index")
        );
    }

    #[test]
    fn dates_are_days_in_utc() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_789_221_603 * 1_000_000_000), "2026-09-12");
        assert_eq!(date(951_782_400 * 1_000_000_000), "2000-02-29");
        assert_eq!(date(-1), "1969-12-31");
    }

    #[test]
    fn vectors_are_made_length_one() {
        let vector = normalized(&[3.0, 4.0]);
        assert!((vector[0] - 0.6).abs() < 1e-6 && (vector[1] - 0.8).abs() < 1e-6);
        assert_eq!(normalized(&[0.0, 0.0]), [0.0, 0.0]);
    }
}
