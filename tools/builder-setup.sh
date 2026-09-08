#!/usr/bin/env bash
# makes any x86-64 linux box (mini pc, old desktop, vps) the eclipse builder. run it on that box:
#
#   curl -fsSL https://raw.githubusercontent.com/officialthomasguthrie/Eclipse/main/tools/builder-setup.sh | bash
#
# installs nix, trusts your user for remote builds, makes sure sshd is up, prints what to run on the mac.| bash
#   (or copy the file over and run: bash builder-setup.sh)
#
# It installs Nix, marks your user as trusted so remote builds are accepted, makes sure SSH is on,
# and prints the line to paste into the Mac's /etc/nix/machines (tools/mac-use-builder.sh does that).
set -euo pipefail

if [[ "$(uname -m)" != "x86_64" ]]; then
  echo "This machine is $(uname -m); the builder must be x86_64." >&2
  exit 1
fi

if ! command -v nix >/dev/null 2>&1; then
  echo ">> Installing Nix (Determinate installer)"
  curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix \
    | sh -s -- install linux --no-confirm
  # shellcheck disable=SC1091
  . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
fi

echo ">> Trusting $USER for remote builds"
sudo mkdir -p /etc/nix
if ! grep -q "trusted-users.*$USER" /etc/nix/nix.custom.conf 2>/dev/null; then
  echo "trusted-users = root $USER" | sudo tee -a /etc/nix/nix.custom.conf >/dev/null
fi
if ! grep -q 'system-features' /etc/nix/nix.custom.conf 2>/dev/null; then
  echo "system-features = nixos-test benchmark big-parallel kvm" | sudo tee -a /etc/nix/nix.custom.conf >/dev/null
fi
sudo systemctl restart nix-daemon 2>/dev/null || true

echo ">> Making sure SSH is on"
if ! systemctl is-active --quiet ssh 2>/dev/null && ! systemctl is-active --quiet sshd 2>/dev/null; then
  sudo apt-get install -y openssh-server >/dev/null 2>&1 || true
  sudo systemctl enable --now ssh 2>/dev/null || sudo systemctl enable --now sshd 2>/dev/null || true
fi
mkdir -p ~/.ssh && chmod 700 ~/.ssh && touch ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys

ip=$(hostname -I 2>/dev/null | awk '{print $1}')
cores=$(nproc)
cat <<MSG

Builder ready.

  user:   $USER
  host:   ${ip:-<this machine's address>}
  cores:  $cores
  kvm:    $([[ -e /dev/kvm ]] && echo yes || echo "no (nix run .#vm will be slow here; builds are fine)")

On the Mac, run:

  ! tools/mac-use-builder.sh $USER@${ip:-<host>} $cores

Then paste the public key it prints into this machine's ~/.ssh/authorized_keys.
MSG
