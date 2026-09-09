# Umbra

The compositor. It starts life as a hard fork of niri (GPL-3.0, Rust, Smithay) and diverges from there.

Empty on purpose until then. To bring the fork in:

    git subtree add --prefix crates/umbra https://github.com/YaLTeR/niri main --squash

Then add crates/umbra to the workspace members, rename the binary to umbra, and point
nix/modules/umbra.nix at it instead of the nixpkgs niri stand-in.

What gets added, in order: output profiles from Syzygy, the session journal for teleport, private
protocols for Corona and Aura, the quake console, the lock screen, the Ghost mode banner.
