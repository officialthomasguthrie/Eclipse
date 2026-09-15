# Horizon

The compositor. A hard fork of niri (GPL-3.0, Rust, Smithay) by Ivan Molodetskikh and the niri
contributors, taken from https://github.com/niri-wm/niri at commit dd75865f on 2026-08-21. It
diverges from there and does not track upstream.

The package and the binary are called horizon. The library crate and the two helper crates keep
their niri names for now, the test snapshots are keyed on them. What was dropped from upstream:
its wiki, CI, packaging files, flake and the visual test app. The rest is as upstream shipped it.
Config docs for the format this still reads are in upstream's wiki at that commit.

The flake builds it as the horizon package, on Linux only. It needs libinput, seatd, libxkbcommon,
gbm, egl, libdisplay-info, pipewire, pango, dbus and systemd at build time.

In the image greetd starts `horizon --session` on tty1 as the owner, with the system config from
/etc/niri/config.kdl. A file at ~/.config/niri/config.kdl replaces it for that user. The desktop
is a flat gray, the boot test takes a screendump of it on a virtio gpu.

What gets added, in order: output profiles from Orbit, the session journal for teleport, private
protocols for Lens and Quasar, the quake console, the lock screen, the Ghost mode banner.
