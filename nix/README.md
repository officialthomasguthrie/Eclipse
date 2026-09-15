# nix/

kernel/     Kernel config.
image/      Partition layout, verity store, persist mounts. ab-sysupdate.nix is the A/B layout, not imported yet.
modules/    One module per component, rift.<name>.enable.
profiles/   base.nix for every image, phase0.nix for the console-only build.
totality/   Boot splash. The PNGs come from tools/gen-totality-assets.py.

test-flatpak.nix is a flatpak runtime and app of our own, as bundles, for the boot test.

image/default.nix follows the repart verity-store appliance layout from nixpkgs
(nixos/tests/appliance-repart-image-verity-store.nix). It builds in CI (rift_0.1.0.raw, about 4.9G)
and the boot job boots it in QEMU: rift-flash writes it onto a drive in a file,
tools/boot-test.py unlocks it over serial and checks the shell. Not booted on real hardware yet.
