#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Checks that every commit of fxdax's Firecracker series builds both ways, as the series promises
# its users, who take it onto forks of their own (TASK-269): without the `fxdax` feature, in a
# checkout outside this repo, as upstream's builds, with the series' own Cargo.lock as it is; and
# with it, as fxdax builds it (scripts/firecracker-cargo.sh). Clippy, its warnings errors, of every
# target: a gate that leaves code unused one way or the other fails it. The tip's build with the
# feature takes vendor/firecracker.lock as it is; the commits' before it may resolve it otherwise,
# their dependencies fewer. The general fixes at the series' base come before the feature does:
# those build as fxdax builds them, with its crate and lock, but no feature to ask for.
#
# Usage: firecracker-series-check.sh [BASE [TIP]]
# BASE defaults to the upstream release the series is on, by commit, as vendor/firecracker.base
# records it: no tag, which a fresh checkout of the fork may lack (the fork had no v1.16.1 tag, and
# git describe found an older one, CI runs 42542801715 and 42547087497). TIP defaults to
# vendor/firecracker's HEAD. FXDAX_FIRECRACKER_SERIES_WORK is where the checkout outside this repo
# and the builds go (default $TMPDIR/fxdax-firecracker-series), kept from run to run.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
fc=$root/vendor/firecracker
work=${FXDAX_FIRECRACKER_SERIES_WORK:-${TMPDIR:-/tmp}/fxdax-firecracker-series}
tip=$(git -C "$fc" rev-parse "${2:-HEAD}")
base=${1:-$(sed -n 's/^\([0-9a-f]\{40\}\) .*/\1/p' "$root/vendor/firecracker.base")}
[[ -n $base ]] || { echo "vendor/firecracker.base names no commit" >&2; exit 2; }
git -C "$fc" cat-file -e "$base^{commit}" 2> /dev/null ||
    { echo "$base: not in vendor/firecracker's history (git -C vendor/firecracker fetch --unshallow)" >&2; exit 2; }
git -C "$fc" merge-base --is-ancestor "$base" "$tip" ||
    { echo "$base is not below ${tip:0:12}: not the series' base" >&2; exit 2; }
echo "The series: $base..${tip:0:12}"
case "$work/" in
    "$root"/*) echo "FXDAX_FIRECRACKER_SERIES_WORK $work is in $root: the default build must be outside it" >&2; exit 2 ;;
esac

[[ -z $(git -C "$fc" rev-list --merges "$base..$tip") ]] || { echo "$base..$tip has merges: not linear" >&2; exit 1; }
mapfile -t commits < <(git -C "$fc" rev-list --reverse "$base..$tip")
[[ ${#commits[@]} -gt 0 ]] || { echo "$base..$tip: no commits" >&2; exit 1; }

mkdir -p "$work"
checkout=$work/firecracker
if [[ ! -e $checkout/.git ]]; then
    # One a container's run left registered, its directory gone with it.
    git -C "$fc" worktree prune
    git -C "$fc" worktree add --detach "$checkout" "$tip"
fi

failed=0
for commit in "${commits[@]}"; do
    git -C "$checkout" checkout -q --detach "$commit"
    subject=$(git -C "$fc" log -1 --format='%h %s' "$commit")
    log=$work/$(git -C "$fc" rev-parse --short "$commit").log
    unlocked=1
    [[ $commit != "$tip" ]] || unlocked=
    feature=(--features fxdax) ways="both ways"
    if ! git -C "$checkout" grep -q '^fxdax = ' -- src/vmm/Cargo.toml; then
        feature=() ways="no fxdax feature yet"
    fi
    if (cd "$checkout" && CARGO_TARGET_DIR=$work/default cargo clippy --locked --workspace \
        --all-targets -- -D warnings) > "$log" 2>&1 &&
        FXDAX_FIRECRACKER_SRC=$checkout FXDAX_FIRECRACKER_TREE=$root/target/firecracker-series \
            FXDAX_FIRECRACKER_UNLOCKED=$unlocked CARGO_TARGET_DIR=$work/fxdax \
            "$root/scripts/firecracker-cargo.sh" clippy -p vmm -p firecracker --all-targets \
            "${feature[@]}" -- -D warnings >> "$log" 2>&1; then
        echo "ok     $subject ($ways)"
    else
        echo "FAILED $subject: $log"
        failed=1
    fi
done
exit $failed
