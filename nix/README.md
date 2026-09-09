# nix/

kernel/     kernel config
image/      partition layout, verity store, persist mounts. ab-sysupdate.nix is the a/b layout, not imported yet
modules/    one module per component, eclipse.<name>.enable
profiles/   base.nix for every image, phase0.nix for the console-only build
totality/   boot splash. the pngs come from tools/gen-totality-assets.py

image/default.nix follows the repart verity-store appliance layout from nixpkgs
(nixos/tests/appliance-repart-image-verity-store.nix). it builds in ci (eclipse_0.1.0.raw, about 4.9G) and the boot job boots it in qemu: tools/persist-image.sh adds
the persist partition, tools/boot-test.py unlocks it over serial and checks the shell. not booted on real hardware yet.
