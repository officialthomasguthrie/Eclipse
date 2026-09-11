//! Who the session belongs to, read from the password file.

/// The account the lock screen asks the password of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    /// The login name PAM checks the password for.
    pub user: String,
    /// The name a person reads: the full name from the comment field, or the login name.
    pub name: String,
}

/// The account with `uid` in the text of a password file.
#[must_use]
pub fn find(passwd: &str, uid: u32) -> Option<Owner> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let user = fields.next().filter(|user| !user.is_empty())?;
        let id: u32 = fields.nth(1)?.parse().ok()?;
        if id != uid {
            return None;
        }
        // the comment field is the full name, then office and phone after commas
        let full = fields
            .nth(1)
            .unwrap_or("")
            .split(',')
            .next()
            .unwrap_or("")
            .trim();
        Some(Owner {
            user: user.to_string(),
            name: if full.is_empty() { user } else { full }.to_string(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0:System administrator:/root:/bin/sh\n\
        eclipse:x:1000:100:Eclipse owner:/home/eclipse:/run/current-system/sw/bin/fish\n\
        guest:x:1001:100::/home/guest:/bin/sh\n\
        office:x:1002:100:Ada Lovelace,Room 4,555:/home/office:/bin/sh\n";

    #[test]
    fn the_full_name_is_what_a_person_reads() {
        assert_eq!(
            find(PASSWD, 1000),
            Some(Owner {
                user: "eclipse".into(),
                name: "Eclipse owner".into()
            })
        );
    }

    #[test]
    fn an_empty_comment_falls_back_to_the_login_name() {
        assert_eq!(
            find(PASSWD, 1001).map(|owner| owner.name),
            Some("guest".into())
        );
    }

    #[test]
    fn only_the_first_part_of_the_comment_is_the_name() {
        assert_eq!(
            find(PASSWD, 1002).map(|owner| owner.name),
            Some("Ada Lovelace".into())
        );
    }

    #[test]
    fn an_unknown_uid_or_a_broken_line_is_nobody() {
        assert_eq!(find(PASSWD, 4242), None);
        assert_eq!(find("eclipse:x:not-a-number:100::/:/bin/sh\n", 1000), None);
        assert_eq!(find(":x:1000:100::/:/bin/sh\n", 1000), None);
    }
}
