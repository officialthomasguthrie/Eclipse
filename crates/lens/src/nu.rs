//! Nushell, the third interpreter. The image ships nushell as a package and Lens runs that
//! binary as a child process with an argument vector, never through a shell. What it prints
//! comes back as plain text for the field: rows for the result list, one sentence for the
//! error line.

// the list of rows is only drawn by the panel, and the panel is linux only
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::process::Command;

/// The binary, found in PATH. The image pins it to the version these arguments assume.
const NU: &str = "nu";

/// How many characters of a line the field keeps. A table is wider than the list, and a line
/// long enough to matter belongs in a terminal.
const WIDTH: usize = 200;

/// The arguments that run one line: no config, no history, plain errors, a table with no rules
/// drawn around it.
#[must_use]
pub fn argv(line: &str) -> Vec<String> {
    [
        "--no-config-file",
        "--no-history",
        "--error-style",
        "plain",
        "--table-mode",
        "none",
        "--commands",
        line,
    ]
    .iter()
    .map(ToString::to_string)
    .collect()
}

/// Run one line in the user's home. Ok holds what it printed, Err one sentence.
///
/// # Errors
///
/// When nushell is missing, cannot start, or reports a problem with the line.
pub fn run(line: &str) -> Result<String, String> {
    let mut command = Command::new(NU);
    command.args(argv(line)).env("NO_COLOR", "1");
    if let Some(home) = std::env::var_os("HOME") {
        command.current_dir(home);
    }
    let output = command
        .output()
        .map_err(|e| format!("Could not run nushell: {e}"))?;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    if output.status.success() {
        Ok(text(&output.stdout))
    } else {
        Err(reason(&text(&output.stderr)))
    }
}

/// The one sentence in a nushell error report. The report starts with the message, then a
/// snippet of the line with a label under the part that went wrong.
#[must_use]
pub fn reason(report: &str) -> String {
    let after = |line: &str, mark: &str| {
        line.split_once(mark)
            .map(|(_, rest)| rest.trim().to_string())
    };
    let mut message = None;
    let mut label = None;
    for line in report.lines() {
        let line = line.trim();
        if message.is_none() {
            message = line
                .strip_prefix("Error:")
                .map(|rest| rest.trim().to_string());
        }
        if label.is_none() && line.starts_with("label at ") {
            label = after(line, ": ");
        }
    }
    // "External command failed" says nothing the label does not say better
    let sentence = match (message, label) {
        (Some(message), Some(label)) if message == "External command failed" => label,
        (Some(message), _) => message,
        (None, Some(label)) => label,
        (None, None) => report
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("Nushell could not run that line")
            .trim()
            .to_string(),
    };
    plain(&sentence)
}

/// The lines of an output, at most `limit` of them, each one short enough for the list.
#[must_use]
pub fn rows(output: &str, limit: usize) -> Vec<String> {
    let mut rows: Vec<String> = output
        .lines()
        // the table indents every line by one space and pads the last column out to its width
        .map(|line| clip(line.trim_end().strip_prefix(' ').unwrap_or(line)))
        .collect();
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    rows.truncate(limit);
    rows
}

/// Cut a line to the width the list draws.
fn clip(line: &str) -> String {
    line.chars().take(WIDTH).collect()
}

/// Nushell writes command names in backticks. The field is not markup.
fn plain(sentence: &str) -> String {
    sentence.replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_is_one_argument() {
        let args = argv("ls | where size > 1kb");
        assert_eq!(args.last().unwrap(), "ls | where size > 1kb");
        assert!(args.contains(&"--no-config-file".to_string()));
        assert!(args.contains(&"plain".to_string()));
    }

    #[test]
    fn a_missing_column_reads_as_itself() {
        let report = "Error: Cannot find column 'nope'\n    \
             Diagnostic severity: error\n\
             Begin snippet for source starting at line 1, column 1\n\n\
             snippet line 1: ls | where nope > 3\n    \
             label at line 1, columns 12 to 15: column 'nope' is missing in one or more values\n";
        assert_eq!(reason(report), "Cannot find column 'nope'");
    }

    #[test]
    fn a_missing_command_reads_as_its_label() {
        let report = "Error: External command failed\n    \
             Diagnostic severity: error\n\
             snippet line 1: frobnicate\n    \
             label at line 1, columns 1 to 10: Command `frobnicate` not found\n\
             diagnostic help: Did you mean `rotate`?\n";
        assert_eq!(reason(report), "Command frobnicate not found");
    }

    #[test]
    fn a_report_with_no_error_line_still_says_something() {
        assert_eq!(reason(""), "Nushell could not run that line");
        assert_eq!(reason("nu: killed\n"), "nu: killed");
    }

    #[test]
    fn rows_lose_the_table_indent_and_the_padding() {
        let output = " 0   one   \n 1   two   \n 2   three \n";
        assert_eq!(rows(output, 8), ["0   one", "1   two", "2   three"]);
    }

    #[test]
    fn rows_stop_at_the_limit_and_drop_the_blank_end() {
        let output = "a\nb\nc\nd\n\n\n";
        assert_eq!(rows(output, 2), ["a", "b"]);
        assert_eq!(rows(output, 8), ["a", "b", "c", "d"]);
        assert!(rows("\n\n", 8).is_empty());
    }

    #[test]
    fn a_long_row_is_cut_to_the_width() {
        let long = "x".repeat(500);
        assert_eq!(rows(&long, 8)[0].chars().count(), WIDTH);
    }
}
