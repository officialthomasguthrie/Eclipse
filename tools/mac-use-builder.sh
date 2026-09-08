#!/usr/bin/env bash
# points this mac at a remote x86-64 builder set up with tools/builder-setup.sh
#
#   sudo tools/mac-use-builder.sh user@host [cores]
#
# nix-daemon runs as root so root needs its own ssh key. writes /etc/nix/machines.
set -euo pipefail

target=${1:?user@host}
cores=${2:-8}
[[ $EUID -eq 0 ]] || { echo "run with sudo (or as ! sudo tools/mac-use-builder.sh ...)" >&2; exit 1; }

host=${target#*@}
if [[ ! -f /var/root/.ssh/id_ed25519 ]]; then
  echo ">> Creating root's SSH key"
  mkdir -p /var/root/.ssh && chmod 700 /var/root/.ssh
  ssh-keygen -t ed25519 -N "" -C "nix-builder@$(hostname -s)" -f /var/root/.ssh/id_ed25519 >/dev/null
fi

echo ">> Recording the builder's host key"
ssh-keyscan -H "$host" 2>/dev/null >> /var/root/.ssh/known_hosts || true

line="ssh-ng://$target x86_64-linux /var/root/.ssh/id_ed25519 $cores 1 kvm,big-parallel,nixos-test,benchmark - -"
mkdir -p /etc/nix
touch /etc/nix/machines
grep -qF "ssh-ng://$target " /etc/nix/machines || echo "$line" >> /etc/nix/machines
grep -q '^builders-use-substitutes' /etc/nix/nix.custom.conf 2>/dev/null \
  || echo "builders-use-substitutes = true" >> /etc/nix/nix.custom.conf
launchctl kickstart -k system/org.nixos.nix-daemon 2>/dev/null \
  || launchctl kickstart -k system/systems.determinate.nix-daemon 2>/dev/null || true

cat <<MSG

Wrote /etc/nix/machines:
  $line

Add root's public key to $target's ~/.ssh/authorized_keys:

$(cat /var/root/.ssh/id_ed25519.pub)

Then test with:
  sudo ssh -i /var/root/.ssh/id_ed25519 $target nix --version
  nix build .#image -L
MSG
