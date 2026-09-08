# umbra

the compositor. it starts life as a hard fork of niri (gpl-3.0, rust, smithay) and diverges from there.

empty on purpose until then. to bring the fork in:

    git subtree add --prefix crates/umbra https://github.com/YaLTeR/niri main --squash

then add crates/umbra to the workspace members, rename the binary to umbra, and point
nix/modules/umbra.nix at it instead of the nixpkgs niri stand-in.

what gets added, in order: output profiles from syzygy, the session journal for teleport, private
protocols for corona and aura, the quake console, the lock screen, the ghost mode banner.
