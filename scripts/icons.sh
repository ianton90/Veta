#!/usr/bin/env bash
# Regenerates crates/veta-gui/assets/lucide-subset.ttf from the full Lucide
# font (lucide-static package, font/lucide.ttf) with every codepoint listed
# in crates/veta-gui/src/icon.rs. Needs fonttools (pyftsubset).
#   scripts/icons.sh path/to/lucide.ttf
set -euo pipefail
cd "$(dirname "$0")/.."
codes=$(grep -o 'Icon::[A-Za-z]* => 0x[0-9a-f]*' crates/veta-gui/src/icon.rs | sed 's/.*0x/U+/' | sort -u | paste -sd, -)
pyftsubset "$1" --unicodes="$codes" --output-file=crates/veta-gui/assets/lucide-subset.ttf
ls -l crates/veta-gui/assets/lucide-subset.ttf
