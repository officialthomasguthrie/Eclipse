# Umbra

The compositor. A hard fork of niri (GPL-3.0, Rust, Smithay) by Ivan Molodetskikh and the niri
contributors, taken from https://github.com/niri-wm/niri at commit dd75865f on 2026-08-21. It
diverges from there and does not track upstream.

The package and the binary are called umbra. The library crate and the two helper crates keep
their niri names for now, the test snapshots are keyed on them. What was dropped from upstream:
its wiki, CI, packaging files, flake and the visual test app. The rest is as upstream shipped it.
Config docs for the format this still reads are in upstream's wiki at that commit.

The flake builds it as the umbra package, on Linux only. It needs libinput, seatd, libxkbcommon,
gbm, egl, libdisplay-info, pipewire, pango, dbus and systemd at build time.

What gets added, in order: the session start on tty1, output profiles from Syzygy, the session
journal for teleport, private protocols for Corona and Aura, the quake console, the lock screen,
the Ghost mode banner.
