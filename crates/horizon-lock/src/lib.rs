//! The lock screen's parts that need no session: who the owner is, the password as it is typed,
//! and what the screen draws. The binary puts them on the lock surfaces and asks PAM.

pub mod draw;
pub mod entry;
pub mod owner;
