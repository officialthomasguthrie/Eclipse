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
            # boots a raw image in qemu. `nix run .#vm` hands it the image above, the boot test builds
            # this package and hands it the image from the artifact. the drive is nvme, not an emulated
            # usb stick: qemu's usb storage returns bad blocks now and then and verity refuses them.
            vm = pkgs.writeShellApplication {
              name = "eclipse-vm";
              runtimeInputs = with pkgs; [
                qemu_kvm
                gptfdisk
                cryptsetup
                btrfs-progs
                util-linux
              ];
              text = ''
                usage() {
                  echo "usage: eclipse-vm [--image file.raw] [--persist passfile] [qemu options]" >&2
                  echo "  --image    the raw image to boot. a read-only file is copied first" >&2
                  echo "  --persist  add the persist partition with the passphrase in this file (sudo)" >&2
                  echo "  the rest goes to qemu after the defaults, so later -m, -smp, -cpu win." >&2
                  echo "  -serial and -display are only set when you pass none" >&2
                }
                image=""
                persist=""
                while [ $# -gt 0 ]; do
                  case $1 in
                    --image) image=''${2:?--image needs a file}; shift 2 ;;
                    --persist) persist=''${2:?--persist needs a passfile}; shift 2 ;;
                    -h|--help) usage; exit 0 ;;
                    --) shift; break ;;
                    *) break ;;
                  esac
                done
                [ -n "$image" ] || { usage; exit 1; }
                [ -f "$image" ] || { echo "eclipse-vm: no such image: $image" >&2; exit 1; }

                work=$(mktemp -d -t eclipse-vm.XXXXXX)
                qemu=""
                # qemu runs as a child so the copy goes away when it ends or when we are killed
                cleanup() {
                  if [ -n "$qemu" ]; then
                    kill "$qemu" 2>/dev/null || true
                    wait "$qemu" || true
                  fi
                  rm -rf "$work"
                }
                trap cleanup EXIT
                trap 'exit 1' HUP INT TERM
                copy=0
                case $image in /nix/store/*) copy=1 ;; esac
                if [ ! -w "$image" ]; then copy=1; fi
                if [ $copy = 1 ]; then
                  echo "eclipse-vm: $image is read-only, copying it to $work" >&2
                  cp --reflink=auto "$image" "$work/eclipse.raw"
                  chmod u+w "$work/eclipse.raw"
                  image=$work/eclipse.raw
                fi
                cp ${pkgs.OVMF.fd}/FV/OVMF_VARS.fd "$work/vars.fd"
                chmod u+w "$work/vars.fd"

                if [ -n "$persist" ]; then
                  echo "eclipse-vm: adding the persist partition through a loop device, sudo may ask for your password" >&2
                  sudo env PATH="$PATH" ${./tools}/persist-image.sh "$image" "$persist"
                fi

                args=(-machine q35 -smp 4 -m 4096)
                if [ -w /dev/kvm ]; then
                  args+=(-accel kvm -cpu host)
                else
                  echo "eclipse-vm: no /dev/kvm, using tcg, this is slow" >&2
                  args+=(-accel tcg -cpu max)
                fi
                serial=1
                display=1
                for a in "$@"; do
                  case $a in
                    -serial) serial=0 ;;
                    -display | -nographic) display=0 ;;
                  esac
                done
                if [ $serial = 1 ]; then args+=(-serial mon:stdio); fi
                if [ $display = 1 ]; then args+=(-display none); fi

                echo "eclipse-vm: booting $image as an nvme drive" >&2
                qemu-system-x86_64 "''${args[@]}" \
                  -drive if=pflash,format=raw,readonly=on,file=${pkgs.OVMF.fd}/FV/OVMF_CODE.fd \
                  -drive if=pflash,format=raw,file="$work/vars.fd" \
                  -drive if=none,id=disk0,format=raw,file="$image" \
                  -device nvme,drive=disk0,serial=eclipse \
                  "$@" <&0 &
                qemu=$!
                wait "$qemu"
              '';
            };
          };

          apps = lib.optionalAttrs isImageHost {
            # nix run .#vm -- [--persist passfile] [qemu options]. add -display gtk to see the splash
            vm = {
              type = "app";
              program = toString (
                pkgs.writeShellScript "eclipse-vm-app" ''
                  exec ${lib.getExe self'.packages.vm} --image ${self'.packages.image}/*.raw "$@"
                ''
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
