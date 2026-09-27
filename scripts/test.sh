#!/usr/bin/env bash
# Run the test suite. Arguments are passed to `cargo nextest run`.
#
# Unicode tests compare against the Unicode Character Database, which is
# downloaded and checksummed on first use.
set -euo pipefail

VERSION=12.1.0
SHA256=25ba51a0d4c6fa41047b7a5e5733068d4a734588f055f61e85f450097834a0a6
root="$(cd "$(dirname "$0")/.." && pwd)"
zip="$root/target/UCD-$VERSION.zip"

if [ ! -f "$zip" ]; then
    mkdir -p "$root/target"
    curl -fsSL -o "$zip" "https://www.unicode.org/Public/$VERSION/ucd/UCD.zip"
fi
echo "$SHA256  $zip" | shasum -a 256 -c - >/dev/null

UCD_ZIP="$zip" cargo nextest run --manifest-path "$root/Cargo.toml" --workspace "$@"
