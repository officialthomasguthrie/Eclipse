#!/usr/bin/env bash
# adds slot b and a persist partition to a raw image file, the way flash.sh does on a stick. the boot test uses it.
#
# Usage: sudo tools/persist-image.sh [--models <dir>] <image.raw> <passfile> [size]   (default size 2G)
# add-slot-b.sh grows the file for the second slot. Then the file grows by <size>, the persist partition takes
# the new space, and mkpersist.sh formats it
# through a loop device. The passphrase comes from a file so nothing is interactive. --models puts
# the files in <dir> into the @models subvolume (see mkpersist.sh).
set -euo pipefail

models=""
if [[ ${1:-} == --models ]]; then
  models=$(readlink -f "${2:?--models needs a directory}")
  shift 2
fi
image=${1:?image.raw}
passfile=${2:?passfile}
size=${3:-2G}
here=$(dirname "$(readlink -f "$0")")

[[ -w "$image" ]] || { echo "not a writable file: $image" >&2; exit 1; }
[[ -r "$passfile" ]] || { echo "no such passfile: $passfile" >&2; exit 1; }
[[ -z "$models" || -d "$models" ]] || { echo "not a directory: $models" >&2; exit 1; }
for tool in sgdisk sfdisk losetup truncate; do
  command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 1; }
done
if sgdisk -p "$image" | grep -q ' persist$'; then
  echo "$image already has a persist partition" >&2
  exit 1
fi

"$here/add-slot-b.sh" "$image"
echo ">> Growing $image by $size"
truncate -s "+$size" "$image"
echo ">> Moving the GPT backup header to the end, adding persist"
sgdisk -e "$image" >/dev/null
sgdisk -n 0:0:0 -t 0:8300 -c 0:persist "$image" >/dev/null
num=$(sgdisk -p "$image" | awk '$NF == "persist" { print $1 }')

loop=$(losetup -Pf --show "$image")
trap 'losetup -d "$loop"' EXIT
part="${loop}p${num}"
for _ in $(seq 20); do
  [[ -b "$part" ]] && break
  sleep 0.5
done
[[ -b "$part" ]] || { echo "$part never appeared" >&2; exit 1; }
if [[ -n "$models" ]]; then
  "$here/mkpersist.sh" --models "$models" "$part" "$passfile"
else
  "$here/mkpersist.sh" "$part" "$passfile"
fi
sync
echo ">> Done: persist is partition $num of $image"
