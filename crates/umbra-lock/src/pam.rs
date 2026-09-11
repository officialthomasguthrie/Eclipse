//! The owner's password against PAM, through the umbra-lock service the image writes to
//! /etc/pam.d.

use std::ffi::{OsStr, OsString};

use nonstick::{AuthnFlags, ConversationAdapter, ErrorCode, Transaction, TransactionBuilder};
use umbra_lock::entry::Verdict;

const SERVICE: &str = "umbra-lock";

/// What PAM's modules get when they ask: the login name, and the password for anything masked.
struct Answers {
    user: String,
    password: String,
}

impl ConversationAdapter for Answers {
    fn prompt(&self, _request: impl AsRef<OsStr>) -> nonstick::Result<OsString> {
        Ok(OsString::from(&self.user))
    }

    fn masked_prompt(&self, _request: impl AsRef<OsStr>) -> nonstick::Result<OsString> {
        Ok(OsString::from(&self.password))
    }

    fn error_msg(&self, message: impl AsRef<OsStr>) {
        eprintln!("umbra-lock: pam: {}", message.as_ref().to_string_lossy());
    }

    fn info_msg(&self, _message: impl AsRef<OsStr>) {}
}

/// Checks `password` for `user`. Only authentication: the session is open already, so nothing
/// about the account is started again.
pub fn check(user: &str, password: String) -> Verdict {
    let answers = Answers {
        user: user.to_string(),
        password,
    };
    let result = TransactionBuilder::new_with_service(SERVICE)
        .username(user)
        .build(answers.into_conversation())
        .and_then(|mut transaction| transaction.authenticate(AuthnFlags::empty()));
    match result {
        Ok(()) => Verdict::Accepted,
        Err(ErrorCode::AuthenticationError | ErrorCode::MaxTries) => {
            eprintln!("umbra-lock: the password for {user} was refused");
            Verdict::Refused
        }
        Err(other) => {
            eprintln!("umbra-lock: pam could not check the password for {user}: {other}");
            Verdict::Failed
        }
    }
}
