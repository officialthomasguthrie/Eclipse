//! Liftoff's text boot. plymouthd loads this crate as a splash plugin: it draws the logo in characters
//! and the system's name at the top, and under them the console as systemd writes to it while the
//! system starts, with plymouth's questions as the last line. The preview example draws the same
//! frames into pictures.

pub mod console;
pub mod font;
pub mod logo;
pub mod screen;

#[cfg(target_os = "linux")]
mod plymouth;
