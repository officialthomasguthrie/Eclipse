#!/usr/bin/env bash
# Design lint. Fails on banned fonts, css effects, banned words and emoji in text files.
# Usage: tools/design-lint.sh [file...]   With no arguments it checks every tracked file.
# The lists below are copied from the design rules; keep the two in step.
set -u

if [ $# -gt 0 ]; then
  files=("$@")
  for f in "${files[@]}"; do [ -f "$f" ] || { echo "no such file: $f"; exit 2; }; done
else
  cd "$(dirname "$0")/.." || exit 2
  files=()
  while IFS= read -r f; do
    case "$f" in
      *.png|*.jpg|*.jpeg|*.gif|*.ico|*.gguf|*.onnx|*.ttf|*.woff|*.woff2) continue ;;
      LICENSE*|*/LICENSE*|Cargo.lock|flake.lock|tools/design-lint.sh) continue ;;
      # the compositor is forked third party code. its comments and shaders are not a surface
      # anyone sees, only its readme is ours
      crates/horizon/README.md) ;;
      crates/horizon/*) continue ;;
    esac
    files+=("$f")
  done < <(git ls-files)
fi
[ "${#files[@]}" -gt 0 ] || exit 0

fonts='Inter|Space Grotesk|Instrument Serif|Poppins|Montserrat|Outfit|Manrope|Plus Jakarta Sans'
fonts="$fonts|DM Sans|Sora|Urbanist|Lexend|JetBrains Mono|Fira Code|IBM Plex( Sans| Mono| Serif)?"
fonts="$fonts|Playfair|Fraunces|Geist|Satoshi|General Sans|Cabinet Grotesk|Clash Display"

words='seamless|powerful|effortless|supercharged|supercharge|unleash|elevate|next-gen|AI-powered'
words="$words|magic|magical|delightful|journey|vibrant|cutting-edge|robust|leverage|empower"
words="$words|reimagine|frictionless|blazing|lightning-fast|game-changing"
words="$words|Oops|Uh oh|Let'?s|Awesome|Hey there"

# css and code effects. to_uppercase() only counts in Rust sources, that is where UI strings live.
css='gradient\(|backdrop-filter|text-transform:[[:space:]]*uppercase|letter-spacing'
radius='border-radius:[[:space:]]*(9|[1-9][0-9]+)(\.[0-9]+)?px'

rs=()
for f in "${files[@]}"; do case "$f" in *.rs) rs+=("$f") ;; esac; done

# every check prints file:line:match, one per line; the label goes at the end
out=$(
  grep -H -n -o -w -E "$fonts" "${files[@]}" 2>/dev/null | sed 's/$/  (banned font)/'
  grep -H -n -o -w -i -E "$words" "${files[@]}" 2>/dev/null | sed 's/$/  (banned word)/'
  grep -H -n -o -E "$css" "${files[@]}" 2>/dev/null | sed 's/$/  (banned css)/'
  grep -H -n -o -E "$radius" "${files[@]}" 2>/dev/null | sed 's/$/  (border-radius above 8px)/'
  if [ "${#rs[@]}" -gt 0 ]; then
    grep -H -n -o -E 'to_uppercase\(\)' "${rs[@]}" 2>/dev/null | sed 's/$/  (uppercase transform)/'
  fi
  # emoji: anything above the basic plane, or in the U+2600 to U+27BF symbol blocks
  perl -CSD -ne 'print "$ARGV:$.:$1\n" while /([\x{2600}-\x{27BF}]|[^\x{0}-\x{FFFF}])/g; close ARGV if eof' \
    "${files[@]}" 2>/dev/null | sed 's/$/  (emoji)/'
)

if [ -n "$out" ]; then
  printf '%s\n' "$out"
  echo "design lint: $(printf '%s\n' "$out" | wc -l | tr -d ' ') problem(s)"
  exit 1
fi
echo "design lint: ok (${#files[@]} files)"
