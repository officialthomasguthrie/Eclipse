//! What the running system calls itself, from os-release.

/// Where the image keeps it.
pub const PATH: &str = "/etc/os-release";

/// The value of `key` in the text of an os-release file, without its quotes, or `None` when the
/// file does not set it. A key set twice has its last value, as in a shell.
#[must_use]
pub fn value(text: &str, key: &str) -> Option<String> {
    text.lines()
        .rev()
        .find_map(|line| line.trim_start().strip_prefix(key)?.strip_prefix('='))
        .map(unquote)
}

/// A value the way os-release(5) writes it: bare, in single quotes, or in double quotes with a
/// backslash before `"`, `\`, `$` and `` ` ``.
fn unquote(raw: &str) -> String {
    let raw = raw.trim_end();
    if let Some(inner) = raw.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        return inner.to_string();
    }
    let inner = raw
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(raw);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.extend(chars.next());
        } else {
            out.push(c);
        }
    }
    out
}

/// The name `rift --version` gives: `PRETTY_NAME` on Rift, like `Rift 0.1.0`, and
/// `rift` with the version of this build on any other system.
#[must_use]
pub fn name(text: &str) -> String {
    match (value(text, "ID"), value(text, "PRETTY_NAME")) {
        (Some(id), Some(pretty)) if id == "rift" && !pretty.is_empty() => pretty,
        _ => format!("rift {}", crate::VERSION),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RIFT: &str = "NAME=\"Rift\"\nID=rift\nID_LIKE=nixos\n\
                        PRETTY_NAME=\"Rift 0.1.0\"\nIMAGE_ID=rift\n\
                        IMAGE_VERSION=\"0.1.0\"\nANSI_COLOR=\"38;2;93;172;217\"\n";

    #[test]
    fn values_come_without_their_quotes() {
        assert_eq!(value(RIFT, "NAME").as_deref(), Some("Rift"));
        assert_eq!(value(RIFT, "ID").as_deref(), Some("rift"));
        assert_eq!(
            value(RIFT, "ANSI_COLOR").as_deref(),
            Some("38;2;93;172;217")
        );
        assert_eq!(value("X='a b'\n", "X").as_deref(), Some("a b"));
        assert_eq!(
            value("X=\"say \\\"hi\\\" to \\$HOME\"\n", "X").as_deref(),
            Some("say \"hi\" to $HOME")
        );
        assert_eq!(value(RIFT, "HOME_URL"), None);
    }

    #[test]
    fn a_key_is_never_found_inside_a_longer_one() {
        assert_eq!(value("PRETTY_NAME=\"Rift 0.1.0\"\n", "NAME"), None);
        assert_eq!(value("ID_LIKE=nixos\n", "ID"), None);
        assert_eq!(value("ID=rift\nID=other\n", "ID").as_deref(), Some("other"));
    }

    #[test]
    fn the_name_says_rift_only_on_rift() {
        assert_eq!(name(RIFT), "Rift 0.1.0");
        let build = format!("rift {}", crate::VERSION);
        assert_eq!(
            name("ID=nixos\nPRETTY_NAME=\"NixOS 26.11 (Zokor)\"\n"),
            build
        );
        assert_eq!(name("ID=rift\nPRETTY_NAME=\"\"\n"), build);
        assert_eq!(name(""), build);
    }
}
