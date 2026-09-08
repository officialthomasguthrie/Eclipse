# nix/

kernel/     kernel config
image/      partition layout, verity store, persist mounts. ab-sysupdate.nix is the a/b layout, not imported yet
modules/    one module per component, eclipse.<name>.enable
profiles/   base.nix for every image, phase0.nix for the console-only build
totality/   boot splash. the pngs come from tools/gen-totality-assets.py

image/default.nix follows the repart verity-store appliance layout from nixpkgs
(nixos/tests/appliance-repart-image-verity-store.nix). it evaluates but hasn't been built yet.
things only a build will prove: the esp size next to the uki the module puts there, plymouth and the
luks prompt inside the systemd initrd, and the store partition staying under 8G.
