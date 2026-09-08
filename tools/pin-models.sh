#!/usr/bin/env bash
# downloads everything in models/manifest.toml into models/cache and prints sha256 lines
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p models/cache
grep -E '^(url|config_url) = ' models/manifest.toml | sed -E 's/^[a-z_]+ = "(.*)"/\1/' | while read -r url; do
  file="models/cache/$(basename "$url")"
  if [[ ! -s "$file" ]]; then
    echo ">> $url" >&2
    curl -fL --retry 3 -o "$file" "$url"
  fi
  printf '%s  %s\n' "$(sha256sum "$file" | cut -d' ' -f1)" "$(basename "$url")"
done
