//! The password field: what has been typed, and the sentence under it.

use std::mem;

/// The longest password the field takes, in characters.
const LONGEST: usize = 512;

/// What the line under the field says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    /// Nothing, the owner is typing.
    #[default]
    Typing,
    /// PAM has the password and has not answered yet.
    Checking,
    /// PAM refused the password.
    Refused,
    /// PAM could not check the password at all.
    Failed,
}

impl Status {
    /// The sentence under the field, when there is one.
    #[must_use]
    pub fn sentence(self) -> Option<&'static str> {
        match self {
            Self::Typing => None,
            Self::Checking => Some("Checking the password."),
            Self::Refused => Some("The password is incorrect. Try again."),
            Self::Failed => Some("The password could not be checked. Try again."),
        }
    }

    /// Whether the sentence reports a problem.
    #[must_use]
    pub fn is_error(self) -> bool {
        matches!(self, Self::Refused | Self::Failed)
    }
}

/// A key press, as the field cares about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key<'a> {
    /// Return: hand the password to PAM.
    Enter,
    /// Backspace: take back the last character.
    Backspace,
    /// Escape: clear the field.
    Escape,
    /// What the key typed, after the keymap.
    Text(&'a str),
}

/// What PAM said about a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// It is the owner's password.
    Accepted,
    /// It is not.
    Refused,
    /// PAM could not tell.
    Failed,
}

/// The field's state.
#[derive(Debug, Default)]
pub struct Entry {
    password: String,
    status: Status,
}

impl Entry {
    /// What the line under the field says.
    #[must_use]
    pub fn status(&self) -> Status {
        self.status
    }

    /// How many characters have been typed. The screen shows one dot for each.
    #[must_use]
    pub fn len(&self) -> usize {
        self.password.chars().count()
    }

    /// Whether nothing has been typed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.password.is_empty()
    }

    /// Takes a key. Returns the password when Enter hands it to PAM, which empties the field.
    /// Keys do nothing while PAM is checking.
    pub fn key(&mut self, key: Key<'_>) -> Option<String> {
        if self.status == Status::Checking {
            return None;
        }
        match key {
            Key::Enter => {
                if self.password.is_empty() {
                    return None;
                }
                self.status = Status::Checking;
                return Some(mem::take(&mut self.password));
            }
            Key::Backspace => {
                self.password.pop();
            }
            Key::Escape => self.password.clear(),
            Key::Text(text) => {
                let room = LONGEST.saturating_sub(self.len());
                let typed: String = text
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(room)
                    .collect();
                if typed.is_empty() {
                    return None;
                }
                self.password.push_str(&typed);
            }
        }
        self.status = Status::Typing;
        None
    }

    /// Takes PAM's answer. Returns whether the session may unlock.
    pub fn checked(&mut self, verdict: Verdict) -> bool {
        self.status = match verdict {
            Verdict::Accepted => return true,
            Verdict::Refused => Status::Refused,
            Verdict::Failed => Status::Failed,
        };
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Entry {
        let mut entry = Entry::default();
        entry.key(Key::Text(text));
        entry
    }

    #[test]
    fn enter_hands_over_what_was_typed_and_empties_the_field() {
        let mut entry = typed("eclipse");
        assert_eq!(entry.len(), 7);
        assert_eq!(entry.key(Key::Enter), Some("eclipse".into()));
        assert!(entry.is_empty());
        assert_eq!(entry.status(), Status::Checking);
    }

    #[test]
    fn enter_on_an_empty_field_does_nothing() {
        let mut entry = Entry::default();
        assert_eq!(entry.key(Key::Enter), None);
        assert_eq!(entry.status(), Status::Typing);
    }

    #[test]
    fn keys_wait_while_pam_checks() {
        let mut entry = typed("eclipse");
        entry.key(Key::Enter);
        assert_eq!(entry.key(Key::Text("x")), None);
        assert_eq!(entry.key(Key::Enter), None);
        assert!(entry.is_empty());
    }

    #[test]
    fn a_refusal_stays_until_the_owner_types_again() {
        let mut entry = typed("wrong");
        entry.key(Key::Enter);
        assert!(!entry.checked(Verdict::Refused));
        assert_eq!(entry.status(), Status::Refused);
        assert_eq!(
            entry.status().sentence(),
            Some("The password is incorrect. Try again.")
        );
        assert!(entry.status().is_error());
        entry.key(Key::Text("e"));
        assert_eq!(entry.status(), Status::Typing);
        assert_eq!(entry.status().sentence(), None);
    }

    #[test]
    fn only_the_right_password_unlocks() {
        let mut entry = typed("eclipse");
        entry.key(Key::Enter);
        assert!(entry.checked(Verdict::Accepted));
        let mut entry = typed("eclipse");
        entry.key(Key::Enter);
        assert!(!entry.checked(Verdict::Failed));
        assert_eq!(entry.status(), Status::Failed);
    }

    #[test]
    fn backspace_escape_and_control_characters() {
        let mut entry = typed("abc");
        entry.key(Key::Backspace);
        assert_eq!(entry.len(), 2);
        entry.key(Key::Text("\u{1b}\t"));
        assert_eq!(entry.len(), 2);
        entry.key(Key::Text("\u{e9}"));
        assert_eq!(entry.len(), 3);
        entry.key(Key::Escape);
        assert!(entry.is_empty());
    }

    #[test]
    fn the_field_stops_at_the_longest_password() {
        let mut entry = typed(&"a".repeat(LONGEST - 1));
        entry.key(Key::Text("bcd"));
        assert_eq!(entry.len(), LONGEST);
        assert_eq!(entry.key(Key::Text("e")), None);
        assert_eq!(entry.len(), LONGEST);
    }
}
