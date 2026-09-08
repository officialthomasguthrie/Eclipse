#!/usr/bin/env bash
# flasher for now, linux only. eclipse-flash replaces this later.
# writes the image, then adds the optional exfat exchange partition and the luks2+btrfs persist partition.
#
# Usage: sudo tools/flash.sh <image.raw> /dev/sdX [exchange-size|none]   (default exchange: 8G)
set -euo pipefail

image=${1:?image.raw}
dev=${2:?target device}
exchange=${3:-8G}

[[ -r "$image" ]] || { echo "no such image: $image" >&2; exit 1; }
[[ -b "$dev" ]] || { echo "not a block device: $dev" >&2; exit 1; }
base=$(basename "$dev")
if [[ "$(cat "/sys/block/$base/removable" 2>/dev/null)" != "1" && "${FORCE:-}" != "1" ]]; then
  echo "$dev is not a removable drive. Eclipse never touches internal disks. Set FORCE=1 if you are sure." >&2
  exit 1
fi
for tool in dd sgdisk partprobe cryptsetup mkfs.btrfs btrfs; do
  command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 1; }
done

echo ">> Writing $image to $dev"
dd if="$image" of="$dev" bs=4M status=progress conv=fsync
sync

echo ">> Moving the GPT backup header to the end of the drive"
sgdisk -e "$dev"

if [[ "$exchange" != "none" ]]; then
  echo ">> Creating the $exchange exchange partition (exFAT)"
  sgdisk -n "0:0:+$exchange" -t 0:0700 -c 0:exchange "$dev"
fi
echo ">> Creating the persist partition in the remaining space"
sgdisk -n 0:0:0 -t 0:8300 -c 0:persist "$dev"
partprobe "$dev"
sleep 2

if [[ "$exchange" != "none" ]]; then
  command -v mkfs.exfat >/dev/null && mkfs.exfat -L EXCHANGE /dev/disk/by-partlabel/exchange
fi

echo ">> Encrypting persist (LUKS2, argon2id). Pick a passphrase."
cryptsetup luksFormat --type luks2 --pbkdf argon2id --label persist /dev/disk/by-partlabel/persist
cryptsetup open /dev/disk/by-partlabel/persist persist

echo ">> btrfs and subvolumes"
mkfs.btrfs -L persist /dev/mapper/persist
mnt=$(mktemp -d)
mount -o compress=zstd:3,noatime /dev/mapper/persist "$mnt"
for sv in @home @var @flatpak @models @hosts @snapshots; do
  btrfs subvolume create "$mnt/$sv"
done
mkdir -p "$mnt/@var/lib/eclipse"
head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$mnt/@var/lib/eclipse/machine-id"
echo >> "$mnt/@var/lib/eclipse/machine-id"
mkdir -p "$mnt/@home/eclipse"
chown 1000:100 "$mnt/@home/eclipse"
umount "$mnt"
cryptsetup close persist

echo ">> Done. Copy the shipped models into @models before first boot if you want Aura."
