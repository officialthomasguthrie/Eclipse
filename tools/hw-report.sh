#!/usr/bin/env bash
# hardware report. run as root on a booted rift or any linux. writes hw/<vendor>-<model>.md, or stdout with -
set -euo pipefail

vendor=$(dmidecode -s system-manufacturer 2>/dev/null | tr -cd '[:alnum:] ' | tr ' ' '-' | tr '[:upper:]' '[:lower:]')
model=$(dmidecode -s system-product-name 2>/dev/null | tr -cd '[:alnum:] ' | tr ' ' '-' | tr '[:upper:]' '[:lower:]')
slug="${vendor:-unknown}-${model:-unknown}"
out="${1:-$(dirname "$0")/../hw/$slug.md}"

{
  echo "# ${vendor:-unknown} ${model:-unknown}"
  echo
  echo "| | |"
  echo "|---|---|"
  echo "| Date | $(date +%F) |"
  echo "| Rift version | $(cat /etc/os-release 2>/dev/null | sed -n 's/^IMAGE_VERSION=//p') |"
  echo "| Firmware | $(dmidecode -s bios-version 2>/dev/null), secure boot: $(mokutil --sb-state 2>/dev/null | head -1 || echo unknown) |"
  echo "| CPU | $(lscpu | sed -n 's/^Model name: *//p') |"
  echo "| RAM | $(free -h | awk '/Mem:/ {print $2}') |"
  echo "| GPU | $(lspci -nn | grep -Ei 'vga|3d|display' | sed 's/^[^ ]* //' | paste -sd ';') |"
  echo "| Wi-Fi | $(lspci -nn | grep -Ei 'network|wireless' | sed 's/^[^ ]* //' | paste -sd ';') |"
  echo "| Display(s) | $(for e in /sys/class/drm/*/edid; do [[ -s $e ]] && echo -n "$(basename "$(dirname "$e")") "; done) |"
  echo
  echo "## Verdict"
  echo
  echo "Boots: yes. Power-on to shell: $(awk '{printf "%.0f", $1}' /proc/uptime) s of uptime at report time."
  echo
  echo "## PCI"
  echo
  echo '```'
  lspci -nn
  echo '```'
  echo
  echo "## USB"
  echo
  echo '```'
  lsusb
  echo '```'
} > "${out/#-/\/dev\/stdout}"

[[ "$out" == "-" ]] || echo "wrote $out"
