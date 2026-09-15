//! Aura's answer in the list under the field. Asking and reading the reply is librift's
//! `aura` module, which the rift command shares; how the words fit the list is the panel's.

// the rows are only drawn by the panel, and the panel is linux only
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

/// How many characters of an answer go on one row of the list. The list is as wide as the field
/// and an answer is drawn in the UI font, not in a fixed width.
pub const WIDTH: usize = 60;

/// The rows an answer takes in the list: every line wrapped at spaces to `WIDTH` characters,
/// blank lines dropped, at most `limit` rows. A word longer than a row is cut into pieces.
#[must_use]
pub fn rows(answer: &str, limit: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for line in answer.lines() {
        let mut row = String::new();
        let mut used = 0;
        for word in line.split_whitespace() {
            let mut rest = word;
            loop {
                let size = rest.chars().count();
                let room = if used == 0 {
                    WIDTH
                } else {
                    WIDTH.saturating_sub(used + 1)
                };
                if size <= room {
                    if used > 0 {
                        row.push(' ');
                        used += 1;
                    }
                    row.push_str(rest);
                    used += size;
                    break;
                }
                if used > 0 {
                    rows.push(std::mem::take(&mut row));
                    used = 0;
                    continue;
                }
                let cut = rest
                    .char_indices()
                    .nth(WIDTH)
                    .map_or(rest.len(), |(at, _)| at);
                rows.push(rest[..cut].to_string());
                rest = &rest[cut..];
            }
        }
        if used > 0 {
            rows.push(row);
        }
    }
    rows.truncate(limit);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_answer_is_one_row() {
        assert_eq!(rows("Paris.", 8), ["Paris."]);
        assert!(rows("\n \n", 8).is_empty());
    }

    #[test]
    fn a_long_line_wraps_at_spaces() {
        let answer = "Rift runs from a USB drive and keeps everything personal on an \
                      encrypted partition, so the computer it runs on is left as it was.";
        let wrapped = rows(answer, 8);
        assert!(wrapped.len() > 1);
        assert!(wrapped.iter().all(|row| row.chars().count() <= WIDTH));
        assert_eq!(
            wrapped.join(" "),
            answer.split_whitespace().collect::<Vec<_>>().join(" ")
        );
    }

    #[test]
    fn lines_stay_rows_and_the_list_stops_at_the_limit() {
        assert_eq!(rows("One.\n\nTwo.\nThree.", 8), ["One.", "Two.", "Three."]);
        assert_eq!(rows("a\nb\nc\nd", 2), ["a", "b"]);
    }

    #[test]
    fn a_word_longer_than_a_row_is_cut() {
        let long = "x".repeat(WIDTH * 2 + 5);
        let cut = rows(&format!("see {long}"), 8);
        assert_eq!(cut[0], "see");
        assert_eq!(cut[1].chars().count(), WIDTH);
        assert_eq!(cut[2].chars().count(), WIDTH);
        assert_eq!(cut[3], "xxxxx");
    }
}
