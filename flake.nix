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
            ./nix/profiles/apps.nix
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
          # the compositor is built apart from the small crates: it pulls in smithay and a dozen
          # system libraries, and the rest of the workspace should stay cheap to build and check
          firstParty = "--workspace --exclude umbra --exclude niri-config --exclude niri-ipc";
          common = {
            inherit src;
            strictDeps = true;
            pname = "eclipse";
            version = "0.1.0";
            cargoExtraArgs = firstParty;
            # corona's panel links the wayland client library and xkbcommon, the lock screen pam
            nativeBuildInputs = lib.optionals pkgs.stdenv.isLinux [ pkgs.pkg-config ];
            buildInputs = lib.optionals pkgs.stdenv.isLinux [
              pkgs.wayland
              pkgs.libxkbcommon
              pkgs.pam
            ];
          };
          cargoArtifacts = craneLib.buildDepsOnly common;
          # every first-party binary except umbra in one package
          workspace = craneLib.buildPackage (
            common
            // {
              inherit cargoArtifacts;
              doCheck = false;
            }
          );
          # eclipse-flash by itself, and on linux with the programs it runs. it needs none of the
          # desktop's libraries, so the vm app gets it in minutes without the rest of the workspace
          flashCommon = {
            inherit src;
            strictDeps = true;
            pname = "eclipse-flash";
            version = "0.1.0";
            cargoExtraArgs = "-p eclipse-flash";
          };
          eclipseFlash = craneLib.buildPackage (
            flashCommon
            // {
              cargoArtifacts = craneLib.buildDepsOnly flashCommon;
              doCheck = false;
              nativeBuildInputs = lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.makeWrapper ];
              postInstall = lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
                wrapProgram $out/bin/eclipse-flash --prefix PATH : ${
                  lib.makeBinPath [
                    pkgs.util-linux
                    pkgs.cryptsetup
                    pkgs.btrfs-progs
                    pkgs.exfatprogs
                  ]
                }
              '';
            }
          );
          # umbra reads shaders, a cursor image and its default config from next to the sources,
          # the cargo source filter alone would drop them
          umbraSrc = lib.cleanSourceWith {
            src = craneLib.path ./.;
            filter =
              path: type: craneLib.filterCargoSources path type || lib.hasInfix "/crates/umbra" (toString path);
          };
          umbraCommon = {
            src = umbraSrc;
            strictDeps = true;
            pname = "umbra";
            version = "0.1.0";
            cargoExtraArgs = "-p umbra";
            nativeBuildInputs = [
              pkgsRust.rustPlatform.bindgenHook
              pkgs.pkg-config
            ];
            buildInputs = with pkgs; [
              cairo
              dbus
              libGL
              libdisplay-info_0_3
              libinput
              seatd
              libxkbcommon
              libgbm
              pango
              pipewire
              systemd
              wayland
            ];
            # libEGL and libwayland-client are opened at run time, keep them in the rpath
            RUSTFLAGS = toString (
              map (arg: "-C link-arg=" + arg) [
                "-Wl,--push-state,--no-as-needed"
                "-lEGL"
                "-lwayland-client"
                "-Wl,--pop-state"
              ]
            );
            NIRI_BUILD_COMMIT = self.shortRev or self.dirtyShortRev or "unknown";
          };
          umbraArtifacts = craneLib.buildDepsOnly umbraCommon;
          umbra = craneLib.buildPackage (
            umbraCommon
            // {
              cargoArtifacts = umbraArtifacts;
              doCheck = false;
            }
          );
          isImageHost = system == "x86_64-linux";
          os = self.nixosConfigurations.eclipse.config;
          # the image's version with the minor version n on
          bump =
            n:
            let
              v = os.system.image.version;
            in
            lib.mkForce "${lib.versions.major v}.${toString (lib.toInt (lib.versions.minor v) + n)}.0";
          # the same system a minor version on. the boot test installs its update files into slot b
          # and reboots into it
          next = self.nixosConfigurations.eclipse.extendModules {
            modules = [ { system.image.version = bump 1; } ];
          };
          # two minor versions on, with a boot check that always fails. it comes up to the shell but
          # never reaches boot-complete.target, so systemd-boot gives up on it after three boots and
          # the boot test sees the version before it start again
          broken = self.nixosConfigurations.eclipse.extendModules {
            modules = [
              (
                { pkgs, ... }:
                {
                  system.image.version = bump 2;
                  systemd.services.never-good = {
                    description = "Boot check that always fails";
                    serviceConfig = {
                      Type = "oneshot";
                      ExecStart = "${pkgs.coreutils}/bin/false";
                    };
                  };
                  systemd.targets.boot-complete = {
                    requires = [ "never-good.service" ];
                    after = [ "never-good.service" ];
                  };
                }
              )
            ];
          };
        in
        {
          packages = {
            default = workspace;
            inherit workspace;
            eclipse-flash = eclipseFlash;
          }
          // lib.optionalAttrs pkgs.stdenv.isLinux { inherit umbra; }
          // lib.optionalAttrs isImageHost {
            image = os.system.build.image;
            # the files systemd-sysupdate installs the next version from
            update = import ./nix/image/update.nix { inherit (next) config pkgs; };
            # and the version after it, which is never marked good
            broken-update = import ./nix/image/update.nix { inherit (broken) config pkgs; };
            # boots a drive in qemu. `nix run .#vm` hands it the image above, which eclipse-flash first
            # writes onto a drive in a file the way it writes a stick; the boot test does the same with
            # the image from the image job. the drive is nvme, not an emulated usb stick: qemu's usb
            # storage returns bad blocks now and then and verity refuses them.
            vm = pkgs.writeShellApplication {
              name = "eclipse-vm";
              runtimeInputs = [
                pkgs.qemu_kvm
                eclipseFlash
              ];
              text = ''
                usage() {
                  echo "usage: eclipse-vm --image file [--persist passfile | --first-boot] [--models dir] [--exchange size] [qemu options]" >&2
                  echo "  --image       the drive to boot. with --persist or --first-boot, the image (.raw or .raw.zst) to write onto one" >&2
                  echo "  --persist     write the image onto a drive in a file with eclipse-flash (sudo), with the" >&2
                  echo "                passphrase for persist in this file" >&2
                  echo "  --first-boot  write the image onto a drive in a file with eclipse-flash (sudo) without persist," >&2
                  echo "                which the drive makes when it first starts, with a passphrase typed there" >&2
                  echo "  --models      copy the files in this directory into the models subvolume (with --persist)" >&2
                  echo "  --exchange    give the drive an exchange partition of this size, like 1G (with --persist or --first-boot)" >&2
                  echo "  the rest goes to qemu after the defaults, so later -m, -smp, -cpu win." >&2
                  echo "  -serial and -display are only set when you pass none" >&2
                }
                image=""
                persist=""
                firstboot=""
                models=""
                exchange=""
                while [ $# -gt 0 ]; do
                  case $1 in
                    --image) image=''${2:?--image needs a file}; shift 2 ;;
                    --persist) persist=''${2:?--persist needs a passfile}; shift 2 ;;
                    --first-boot) firstboot=1; shift ;;
                    --models) models=''${2:?--models needs a directory}; shift 2 ;;
                    --exchange) exchange=''${2:?--exchange needs a size}; shift 2 ;;
                    -h|--help) usage; exit 0 ;;
                    --) shift; break ;;
                    *) break ;;
                  esac
                done
                [ -n "$image" ] || { usage; exit 1; }
                [ -f "$image" ] || { echo "eclipse-vm: no such image: $image" >&2; exit 1; }
                if [ -n "$persist" ] && [ -n "$firstboot" ]; then
                  echo "eclipse-vm: --persist and --first-boot do not go together" >&2
                  exit 1
                fi
                if [ -n "$models" ] && [ -z "$persist" ]; then
                  echo "eclipse-vm: --models goes with --persist" >&2
                  exit 1
                fi
                if [ -n "$exchange" ] && [ -z "$persist$firstboot" ]; then
                  echo "eclipse-vm: --exchange goes with --persist or --first-boot" >&2
                  exit 1
                fi
                if [ -n "$models" ] && [ ! -d "$models" ]; then
                  echo "eclipse-vm: no such directory: $models" >&2
                  exit 1
                fi

                work=$(mktemp -d -t eclipse-vm.XXXXXX)
                qemu=""
                # qemu runs as a child so the drive goes away when it ends or when we are killed
                cleanup() {
                  if [ -n "$qemu" ]; then
                    kill "$qemu" 2>/dev/null || true
                    wait "$qemu" || true
                  fi
                  rm -rf "$work"
                }
                trap cleanup EXIT
                trap 'exit 1' HUP INT TERM
                if [ -n "$persist$firstboot" ]; then
                  # a sparse file the size of a small stick. eclipse-flash reads the image where it
                  # is, so one in the store needs no copy
                  flash=(write)
                  if [ -n "$firstboot" ]; then flash+=(--first-boot); fi
                  if [ -n "$models" ]; then flash+=(--models "$models"); fi
                  if [ -n "$exchange" ]; then flash+=(--exchange "$exchange"); fi
                  truncate -s 24G "$work/drive.img"
                  echo "eclipse-vm: writing $image onto a drive in $work with eclipse-flash, sudo may ask for your password" >&2
                  if [ -n "$firstboot" ]; then
                    # no passphrase goes in, the drive asks for one when it first starts
                    sudo "$(command -v eclipse-flash)" "''${flash[@]}" "$image" "$work/drive.img"
                  else
                    # sudo sets a path of its own. the passfile is read as the person running this, not
                    # as root, which is what the redirect is for
                    # shellcheck disable=SC2024
                    sudo "$(command -v eclipse-flash)" "''${flash[@]}" "$image" "$work/drive.img" < "$persist"
                  fi
                  image=$work/drive.img
                else
                  copy=0
                  case $image in /nix/store/*) copy=1 ;; esac
                  if [ ! -w "$image" ]; then copy=1; fi
                  if [ $copy = 1 ]; then
                    echo "eclipse-vm: $image is read-only, copying it to $work" >&2
                    cp --reflink=auto "$image" "$work/drive.img"
                    chmod u+w "$work/drive.img"
                    image=$work/drive.img
                  fi
                fi
                cp ${pkgs.OVMF.fd}/FV/OVMF_VARS.fd "$work/vars.fd"
                chmod u+w "$work/vars.fd"

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
          }
          // lib.optionalAttrs pkgs.stdenv.isLinux { inherit umbra; };

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
