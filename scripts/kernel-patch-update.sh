#!/bin/sh
# Rewrites a patch in scripts/kernel-patches from its source's generated copy in
# vendor/fuchsia-kernel, keeping the patch's description (the lines before its diff).
#
# Usage: scripts/kernel-patch-update.sh PATCH SOURCE
#
# PATCH is the patch's file name, SOURCE the patched file relative to vendor/fuchsia. Edit the
# generated copy, then run this; scripts/kernel-cfg.py --check verifies the result.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
patch="$ROOT/scripts/kernel-patches/$1"
source="$2"

updated="$(mktemp)"
trap 'rm -f "$updated"' EXIT
sed '/^--- a\//,$d' "$patch" > "$updated"
"$ROOT/scripts/kernel-cfg.py" --diff "$source" >> "$updated"
chmod 644 "$updated"
mv "$updated" "$patch"
trap - EXIT
