# task runner, `just` lists these

set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

# the umbra crates need the compositor's system libraries and are built through nix
first_party := "--workspace --exclude umbra --exclude niri-config --exclude niri-ipc"

# Build every first party crate except umbra
build:
    cargo build {{first_party}}

# Run the test suite
test:
    cargo test {{first_party}}

# Format and lint the way CI does
lint:
    cargo fmt --all --check
    cargo clippy {{first_party}} --all-targets -- -D warnings

# Build the compositor (linux only)
umbra:
    nix build .#umbra -L

# Format everything
fmt:
    cargo fmt --all
    nix fmt

# Build the bootable image (needs an x86_64-linux builder)
image:
    nix build .#image -L

# Boot the image in qemu (linux host with kvm)
vm *ARGS:
    nix run .#vm -- {{ARGS}}

# Everything nix knows how to verify
check:
    nix flake check -L

# Regenerate the Totality boot splash assets
splash-assets:
    python3 tools/gen-totality-assets.py nix/totality/plymouth

# Download the models in the manifest and print their hashes
pin-models:
    tools/pin-models.sh

# Write a hardware report for the machine this runs on (Linux, needs root)
hw-report:
    sudo tools/hw-report.sh
