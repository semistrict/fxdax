#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Mutation testing of fxdax's own crates (TASK-44.1): cargo-mutants (MIT) changes
# their code a mutant at a time and runs their tests with cargo nextest; a mutant
# no test fails, missed, is a gap in the tests, or a change that changes nothing.
# Then, for the crates with Shuttle tests (the `shuttle` feature: the concurrency
# rules the store and the daemon rely on), the mutants missed run again with that
# feature, its tests alone: cargo-mutants runs one feature set at a time.
#
#   scripts/mutants.sh [CRATE...]   (default: extent_frame extent_store extent_remote fxdax)
#
# Each crate's report goes to $FXDAX_MUTANTS/<crate>/mutants.out (default:
# ~/.cache/fxdax/mutants): caught.txt, missed.txt, timeout.txt, unviable.txt.
# FXDAX_MUTANTS_JOBS mutants build and run at once (default 3). It takes about a
# day for all four crates: extent_remote's simulation takes some 77 s a mutant,
# and the whole sweep has some 3,600 mutants. Run it on a host of its own
# (scripts/mutants-gce.sh), in shards: FXDAX_MUTANTS_SHARD=K/N runs the Kth of N
# (cargo-mutants' --shard), and the Shuttle pass over its missed.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
crates=("$@")
if (( ${#crates[@]} == 0 )); then
    crates=(extent_frame extent_store extent_remote fxdax)
fi
out=${FXDAX_MUTANTS:-$HOME/.cache/fxdax/mutants}
jobs=${FXDAX_MUTANTS_JOBS:-3}
shard=()
if [[ -n ${FXDAX_MUTANTS_SHARD:-} ]]; then
    [[ $FXDAX_MUTANTS_SHARD =~ ^[0-9]+/[1-9][0-9]*$ ]] || {
        echo 'FXDAX_MUTANTS_SHARD is K/N' >&2
        exit 2
    }
    shard=(--shard "$FXDAX_MUTANTS_SHARD")
fi
command -v cargo-mutants > /dev/null || {
    echo "cargo-mutants is missing: cargo install cargo-mutants --locked" >&2
    exit 2
}
cd "$root"
status=0
for crate in "${crates[@]}"; do
    mkdir -p "$out/$crate"
    echo "== $crate" >&2
    # cargo-mutants exits 2 when it missed mutants, which is a result, not a failure.
    cargo mutants -p "$crate" --test-tool nextest --jobs "$jobs" --output "$out/$crate" \
        ${shard[@]+"${shard[@]}"} || (( $? == 2 )) || status=1
    if grep -q '^shuttle' "crates/$crate/Cargo.toml" &&
        [[ -s "$out/$crate/mutants.out/missed.txt" ]]; then
        echo "== $crate, the missed under Shuttle" >&2
        cargo mutants -p "$crate" --test-tool nextest --jobs "$jobs" --output "$out/$crate" \
            --iterate --features shuttle -- -E 'test(/shuttle/) | binary(shuttle)' ||
            (( $? == 2 )) || status=1
    fi
    for kind in caught missed timeout unviable; do
        file="$out/$crate/mutants.out/$kind.txt"
        echo "$crate $kind $( [[ -f $file ]] && wc -l < "$file" || echo 0)"
    done
done
exit "$status"
