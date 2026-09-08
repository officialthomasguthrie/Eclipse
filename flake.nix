{
  description = "Eclipse OS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{
      self,
      nixpkgs,
      flake-parts,
      crane,
      rust-overlay,
      ...
    }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      flake = {
        # one nixos module per component
        nixosModules = {
          totality = import ./nix/modules/totality.nix;
          umbra = import ./nix/modules/umbra.nix;
          corona = import ./nix/modules/corona.nix;
          aura = import ./nix/modules/aura.nix;
          syzygy = import ./nix/modules/syzygy.nix;
          penumbra = import ./nix/modules/penumbra.nix;
          vault = import ./nix/modules/vault.nix;
          default = {
            imports = builtins.attrValues (builtins.removeAttrs self.nixosModules [ "default" ]);
          };
        };

        # the os. nix build .#image
        nixosConfigurations.eclipse = nixpkgs.lib.nixosSystem {
          specialArgs = { inherit inputs self; };
          modules = [
            { nixpkgs.hostPlatform = "x86_64-linux"; }
            self.nixosModules.default
            ./nix/kernel
            ./nix/image
            ./nix/profiles/base.nix
            ./nix/profiles/phase0.nix
          ];
        };
      };

      perSystem =
        {
          config,
          self',
          pkgs,
          system,
          lib,
          ...
        }:
        let
          pkgsRust = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          toolchain = pkgsRust.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          craneLib = (crane.mkLib pkgsRust).overrideToolchain toolchain;
          src = craneLib.cleanCargoSource ./.;
          common = {
            inherit src;
            strictDeps = true;
            pname = "eclipse";
            version = "0.1.0";
          };
          cargoArtifacts = craneLib.buildDepsOnly common;
          # every first-party binary in one package
          workspace = craneLib.buildPackage (
            common
            // {
              inherit cargoArtifacts;
              doCheck = false;
            }
          );
          isImageHost = system == "x86_64-linux";
          os = self.nixosConfigurations.eclipse.config;
        in
        {
          packages = {
            default = workspace;
            inherit workspace;
          }
          // lib.optionalAttrs isImageHost {
            image = os.system.build.image;
          };

          apps = lib.optionalAttrs isImageHost {
            # boots the image in qemu as a usb stick
            vm = {
              type = "app";
              program = lib.getExe (
                pkgs.writeShellApplication {
                  name = "eclipse-vm";
                  runtimeInputs = [ pkgs.qemu_kvm ];
                  text = ''
                    img=$(ls ${self'.packages.image}/*.raw | head -n 1)
                    work=$(mktemp -d)
                    cp --reflink=auto "$img" "$work/eclipse.raw"
                    cp ${pkgs.OVMF.fd}/FV/OVMF_VARS.fd "$work/vars.fd"
                    chmod u+w "$work/eclipse.raw" "$work/vars.fd"
                    echo "booting $img as a usb device. add -display gtk to see the splash" >&2
                    exec qemu-system-x86_64 -enable-kvm -cpu host -smp 4 -m 4096 \
                      -drive if=pflash,format=raw,readonly=on,file=${pkgs.OVMF.fd}/FV/OVMF_CODE.fd \
                      -drive if=pflash,format=raw,file="$work/vars.fd" \
                      -device qemu-xhci \
                      -drive if=none,id=usb0,format=raw,file="$work/eclipse.raw" \
                      -device usb-storage,drive=usb0 \
                      -serial mon:stdio -display none "$@"
                  '';
                }
              );
            };
          };

          checks = {
            inherit workspace;
            clippy = craneLib.cargoClippy (
              common
              // {
                inherit cargoArtifacts;
                cargoClippyExtraArgs = "--all-targets -- --deny warnings";
              }
            );
            fmt = craneLib.cargoFmt { inherit src; };
            tests = craneLib.cargoTest (common // { inherit cargoArtifacts; });
          };

          devShells.default = craneLib.devShell {
            checks = self'.checks;
            packages =
              with pkgs;
              [
                just
                nushell
                nixfmt-rfc-style
                nil
                cargo-watch
                cargo-nextest
                python3
              ]
              ++ lib.optionals stdenv.isLinux [
                qemu_kvm
                gptfdisk
                cryptsetup
                btrfs-progs
              ];
          };

          formatter = pkgs.nixfmt-rfc-style;
        };
    };
}
