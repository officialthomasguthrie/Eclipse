#!/usr/bin/env bash
# formats one partition as the persist volume: luks2 around btrfs, one subvolume per kind of personal data.
# flash.sh runs this on a stick, persist-image.sh on a loop device for the boot test.
#
# Usage: sudo tools/mkpersist.sh <partition> [passfile]
# Without a passfile cryptsetup asks for a passphrase. A passfile means a throwaway test volume: the
# passphrase is the file's exact contents (no trailing newline) and key derivation is set to the
# cheapest settings so a small vm can open it quickly.
set -euo pipefail

part=${1:?partition}
passfile=${2:-}
name=${PERSIST_NAME:-persist}

[[ -b "$part" ]] || { echo "not a block device: $part" >&2; exit 1; }
for tool in cryptsetup mkfs.btrfs btrfs; do
  command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 1; }
done

if [[ -n "$passfile" ]]; then
  [[ -r "$passfile" ]] || { echo "no such passfile: $passfile" >&2; exit 1; }
  echo ">> Encrypting $part (LUKS2, test settings, passphrase from $passfile)"
  cryptsetup luksFormat --type luks2 --pbkdf argon2id --pbkdf-force-iterations 4 --pbkdf-memory 65536 \
    --label persist --batch-mode --key-file "$passfile" "$part"
  cryptsetup open --key-file "$passfile" "$part" "$name"
else
  echo ">> Encrypting $part (LUKS2, argon2id). Pick a passphrase."
  cryptsetup luksFormat --type luks2 --pbkdf argon2id --label persist "$part"
  cryptsetup open "$part" "$name"
fi

echo ">> btrfs and subvolumes"
mkfs.btrfs -q -L persist "/dev/mapper/$name"
mnt=$(mktemp -d)
mount -o compress=zstd:3,noatime "/dev/mapper/$name" "$mnt"
for sv in @home @var @flatpak @models @hosts @snapshots; do
  btrfs subvolume create "$mnt/$sv" >/dev/null
done
mkdir -p "$mnt/@var/lib/eclipse"
head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$mnt/@var/lib/eclipse/machine-id"
echo >> "$mnt/@var/lib/eclipse/machine-id"
mkdir -p "$mnt/@home/eclipse"
chown 1000:100 "$mnt/@home/eclipse"
umount "$mnt"
rmdir "$mnt"
cryptsetup close "$name"
