//! Reading the XML property lists macOS's diskutil prints, without a crate for it. Only what diskutil
//! uses is kept: dictionaries, arrays, strings, integers and booleans. Data, dates and reals are read
//! over.

/// A value in a property list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    Dict(Vec<(String, Value)>),
    Array(Vec<Value>),
    String(String),
    Integer(i64),
    Bool(bool),
    /// Data, a date or a real.
    Other,
}

impl Value {
    /// The value under `key` in a dictionary.
    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// The string under `key`, trimmed, when it is not empty.
    pub(crate) fn text(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(Value::String(text)) => Some(text.trim()).filter(|text| !text.is_empty()),
            _ => None,
        }
    }

    /// Whether `key` holds true.
    pub(crate) fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some(Value::Bool(true)))
    }

    /// The integer under `key`, when it is one and not negative.
    pub(crate) fn number(&self, key: &str) -> Option<u64> {
        match self.get(key) {
            Some(Value::Integer(number)) => u64::try_from(*number).ok(),
            _ => None,
        }
    }

    /// The array under `key`, or nothing.
    pub(crate) fn items(&self, key: &str) -> &[Value] {
        match self.get(key) {
            Some(Value::Array(items)) => items,
            _ => &[],
        }
    }
}

/// The value a property list holds.
pub(crate) fn read_plist(text: &str) -> Result<Value, String> {
    let start = text.find("<plist").ok_or("It is not a property list.")?;
    let mut reader = Reader { text, at: start };
    reader.tag()?;
    let first = reader.tag()?;
    let value = reader.value(first)?;
    match reader.tag()? {
        "/plist" => Ok(value),
        other => Err(format!("It has <{other}> after its value.")),
    }
}

struct Reader<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Reader<'a> {
    /// The next tag without its angle brackets: `dict`, `/dict`, `true/`.
    fn tag(&mut self) -> Result<&'a str, String> {
        let text: &'a str = self.text;
        let rest = &text[self.at..];
        let trimmed = rest.trim_start();
        let Some(inside) = trimmed.strip_prefix('<') else {
            return Err(if trimmed.is_empty() {
                "It ends before its last tag.".to_string()
            } else {
                "It has text where a tag belongs.".to_string()
            });
        };
        let end = inside.find('>').ok_or("A tag in it is not closed.")?;
        self.at += rest.len() - trimmed.len() + 1 + end + 1;
        Ok(&inside[..end])
    }

    /// The text up to `</name>`, with its entities replaced.
    fn content(&mut self, name: &str) -> Result<String, String> {
        let text: &'a str = self.text;
        let rest = &text[self.at..];
        let close = format!("</{name}>");
        let end = rest
            .find(&close)
            .ok_or_else(|| format!("A <{name}> in it is not closed."))?;
        self.at += end + close.len();
        unescape(&rest[..end])
    }

    fn value(&mut self, tag: &str) -> Result<Value, String> {
        Ok(match tag {
            "dict" => {
                let mut entries = Vec::new();
                loop {
                    match self.tag()? {
                        "/dict" => break,
                        "key" => {
                            let key = self.content("key")?;
                            let next = self.tag()?;
                            entries.push((key, self.value(next)?));
                        }
                        other => {
                            return Err(format!(
                                "A dictionary in it has <{other}> where a key belongs."
                            ));
                        }
                    }
                }
                Value::Dict(entries)
            }
            "array" => {
                let mut items = Vec::new();
                loop {
                    let next = self.tag()?;
                    if next == "/array" {
                        break;
                    }
                    items.push(self.value(next)?);
                }
                Value::Array(items)
            }
            "string" => Value::String(self.content("string")?),
            "integer" => {
                let number = self.content("integer")?;
                Value::Integer(
                    number
                        .trim()
                        .parse()
                        .map_err(|_| format!("{number} in it is not an integer."))?,
                )
            }
            "dict/" => Value::Dict(Vec::new()),
            "array/" => Value::Array(Vec::new()),
            "string/" => Value::String(String::new()),
            "true/" => Value::Bool(true),
            "false/" => Value::Bool(false),
            "real" | "date" | "data" => {
                self.content(tag)?;
                Value::Other
            }
            "real/" | "date/" | "data/" => Value::Other,
            other => return Err(format!("It has a <{other}> tag.")),
        })
    }
}

/// XML text with its entities replaced by the characters they stand for.
fn unescape(text: &str) -> Result<String, String> {
    let mut plain = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        plain.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let semicolon = after.find(';').ok_or("An entity in it is not closed.")?;
        let name = &after[..semicolon];
        let character = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => name
                .strip_prefix("#x")
                .map(|hex| u32::from_str_radix(hex, 16))
                .or_else(|| name.strip_prefix('#').map(str::parse::<u32>))
                .and_then(Result::ok)
                .and_then(char::from_u32),
        };
        plain.push(character.ok_or_else(|| format!("It has an entity &{name}; in it."))?);
        rest = &after[semicolon + 1..];
    }
    plain.push_str(rest);
    Ok(plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>BusProtocol</key>
	<string>USB</string>
	<key>MediaName</key>
	<string>Ultra Fit &amp; &lt;more&gt; &#x41;&#66;</string>
	<key>Size</key>
	<integer>61530439680</integer>
	<key>Internal</key>
	<false/>
	<key>RemovableMedia</key>
	<true/>
	<key>MountPoint</key>
	<string></string>
	<key>Empty</key>
	<string/>
	<key>Stores</key>
	<array>
		<dict>
			<key>DeviceIdentifier</key>
			<string>disk4s2</string>
		</dict>
	</array>
	<key>None</key>
	<array/>
	<key>Seen</key>
	<date>2026-09-13T06:57:03Z</date>
	<key>Blob</key>
	<data>
	AAEC
	</data>
	<key>Ratio</key>
	<real>1.5</real>
	<key>Negative</key>
	<integer>-1</integer>
</dict>
</plist>
"#;

    #[test]
    fn a_property_list_reads_as_its_values() {
        let value = read_plist(SMALL).unwrap();
        assert_eq!(value.text("BusProtocol"), Some("USB"));
        assert_eq!(value.text("MediaName"), Some("Ultra Fit & <more> AB"));
        assert_eq!(value.number("Size"), Some(61_530_439_680));
        assert!(!value.flag("Internal"));
        assert!(value.flag("RemovableMedia"));
        assert!(!value.flag("Missing"));
        assert_eq!(value.text("MountPoint"), None);
        assert_eq!(value.text("Empty"), None);
        assert_eq!(
            value.items("Stores")[0].text("DeviceIdentifier"),
            Some("disk4s2")
        );
        assert!(value.items("None").is_empty());
        assert!(value.items("Missing").is_empty());
        assert_eq!(value.get("Seen"), Some(&Value::Other));
        assert_eq!(value.get("Blob"), Some(&Value::Other));
        assert_eq!(value.number("Negative"), None);
        assert_eq!(value.get("Negative"), Some(&Value::Integer(-1)));
    }

    #[test]
    fn what_is_not_a_property_list_is_refused() {
        for broken in [
            "",
            "diskutil: could not find disk: disk9",
            "<plist version=\"1.0\"><dict><key>A</key>",
            "<plist version=\"1.0\"><dict><string>A</string></dict></plist>",
            "<plist version=\"1.0\"><integer>ten</integer></plist>",
            "<plist version=\"1.0\"><string>&nbsp;</string></plist>",
            "<plist version=\"1.0\"><string>a</string><string>b</string></plist>",
            "<plist version=\"1.0\"><dict>text</dict></plist>",
            "<plist version=\"1.0\"><thing/></plist>",
        ] {
            assert!(read_plist(broken).is_err(), "{broken}");
        }
    }
}
