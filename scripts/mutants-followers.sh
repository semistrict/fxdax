#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Mutation testing of live read-only followers' code (backlog doc-8, Testing; TASK-42):
# cargo-mutants (MIT) changes it a mutant at a time and runs the tests that check it, with cargo
# nextest; a mutant no test fails, missed, needs a test, or is recorded as equivalent, with why.
# Its parts, each against its own tests:
#
# - views: the store's views (crates/extent_store: views.rs, kept.rs, and Store's view and kept
#   functions), against extent_store's tests, then the missed against its Shuttle tests;
# - fxfs: the Fxfs follower (crates/fxfs/src/follower.rs), against its tests and the followers'
#   simulation (follower_sim, under Shuttle);
# - protocol: the concurrency protocol (crates/follower), against its tests, then the missed
#   against its Shuttle tests;
# - history: the history checker (crates/follower_history), against its tests and the
#   simulation's, which check their histories with it.
#
#   scripts/mutants-followers.sh [PART...]   (default: views fxfs protocol history)
#
# Each part's report goes to $FXDAX_MUTANTS/followers-PART/mutants.out (default:
# ~/.cache/fxdax/mutants): caught.txt, missed.txt, timeout.txt, unviable.txt. FXDAX_MUTANTS_JOBS
# mutants build and run at once (default 3). The parts run on macOS, in some 5 to 15 minutes each.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
parts=("$@")
if (( ${#parts[@]} == 0 )); then
    parts=(views fxfs protocol history)
fi
out=${FXDAX_MUTANTS:-$HOME/.cache/fxdax/mutants}
jobs=${FXDAX_MUTANTS_JOBS:-3}
command -v cargo-mutants > /dev/null || {
    echo "cargo-mutants is missing: cargo install cargo-mutants --locked" >&2
    exit 2
}
# Whether cargo-mutants' exit status $1 is a result: 2 when it missed mutants, 3 when some timed out.
result() {
    (( $1 == 2 || $1 == 3 ))
}
# The regex that picks exactly the mutants the file $1 lists, for cargo-mutants' -F.
exactly() {
    python3 -c 'import re, sys
print("|".join("^" + re.escape(line.strip()) + "$" for line in open(sys.argv[1]) if line.strip()))' "$1"
}
# mutants PART ARGS...: cargo-mutants, with ARGS, into PART's report.
mutants() {
    local part=$1
    shift
    cargo mutants --test-tool nextest --jobs "$jobs" --output "$out/followers-$part" "$@"
}
# under_shuttle PART PACKAGE: the mutants of PART missed, again, against PACKAGE's Shuttle tests,
# which its `shuttle` feature builds: cargo-mutants runs one feature set at a time. Exactly the
# missed: --iterate alone would run those of the whole package.
under_shuttle() {
    local missed="$out/followers-$1/mutants.out/missed.txt"
    if [[ -s $missed ]]; then
        echo "== $1, the missed under Shuttle" >&2
        mutants "$1" -p "$2" --iterate --features shuttle -F "$(exactly "$missed")" \
            -- -E 'test(/shuttle/) | binary(shuttle)' || result $? || status=1
    fi
}
cd "$root"
status=0
for part in "${parts[@]}"; do
    mkdir -p "$out/followers-$part"
    echo "== $part" >&2
    case $part in
        views)
            # Store's functions of views and what they keep, and its rules and guards of them:
            # the rest of store.rs is the store's own, which TASK-44.1's sweep covers
            # (cargo-mutants 27.1.0 lets a few of its field deletions through the filters).
            store='Store<D>::(view|view_stats|view_extent|view_reader|read_view|locate_view|newer|cut_views|trim_kept|kept_bytes|read_kept)\b|views_|Reader::View|kept\('
            mutants views -p extent_store -f crates/extent_store/src/views.rs \
                -f crates/extent_store/src/kept.rs -f crates/extent_store/src/store.rs \
                -F "^crates/extent_store/src/(views|kept)\.rs:|$store" || result $? || status=1
            under_shuttle views extent_store
            ;;
        fxfs)
            # The simulation's tests join the mutated package's: only theirs and the follower's
            # unit tests run, each a process of its own, as Shuttle's need.
            mutants fxfs -p fxfs -f crates/fxfs/src/follower.rs --timeout 600 \
                -- --package follower_sim --features follower_sim/shuttle \
                -E 'package(follower_sim) | test(/^follower::tests::/)' || result $? || status=1
            ;;
        protocol)
            mutants protocol -p follower --timeout 600 || result $? || status=1
            under_shuttle protocol follower
            ;;
        history)
            mutants history -p follower_history --timeout 600 \
                -- --package follower_sim --features follower_sim/shuttle \
                -E 'package(follower_history) | package(follower_sim)' || result $? || status=1
            ;;
        *) echo "Unknown part: $part" >&2; exit 2 ;;
    esac
    for kind in caught missed timeout unviable; do
        file="$out/followers-$part/mutants.out/$kind.txt"
        echo "$part $kind $( [[ -f $file ]] && wc -l < "$file" || echo 0)"
    done
done
exit "$status"
