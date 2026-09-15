//! The wayland client library and xkbcommon are opened by name at run time: the workspace builds
//! wayland-sys with dlopen for lens's panel, and features are shared across the workspace. There
//! is no system library path on the image, so the lock screen binary names them as needed
//! libraries, like lens does: the loader finds them through the rpath, and a dlopen by soname
//! then returns the copy that is already loaded.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        for arg in [
            "-Wl,--push-state,--no-as-needed",
            "-lwayland-client",
            "-lxkbcommon",
            "-Wl,--pop-state",
        ] {
            println!("cargo:rustc-link-arg-bins={arg}");
        }
    }
}
