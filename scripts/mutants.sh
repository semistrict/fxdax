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
# ~/.cache/fxdax/mutants): caught.txt, missed.txt, timeout.txt, unviable.txt;
# the Shuttle pass's, of the mutants it ran again, to
# <crate>/shuttle/mutants.out. A rerun keeps each report it replaces as
# mutants.out.old, as cargo-mutants does. Each pass's line follows it: its
# counts, or if it failed, its status, as its report may then be an earlier
# run's, or of some of its mutants; the Shuttle pass runs only after a sweep
# that finished. FXDAX_MUTANTS_JOBS mutants build and run at once (default 3).
# It takes about a day for all four crates: extent_remote's simulation takes
# some 77 s a mutant, and the whole sweep has some 3,600 mutants. Run it on a
# host of its own (scripts/mutants-gce.sh), in shards: FXDAX_MUTANTS_SHARD=K/N
# runs the Kth of N (cargo-mutants' --shard), and the Shuttle pass over its
# missed.
# FXDAX_MUTANTS_LIST names a file of mutants, one a line as `cargo mutants --list`
# names them, to run alone: a sweep's survivors, again, once tests for them came.
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
# Whether cargo-mutants' exit status $1 says it finished: 0, or 2 when it missed mutants, 3 when
# some timed out.
finished() {
    (( $1 == 0 || $1 == 2 || $1 == 3 ))
}
# The counts of each kind of mutant in the report a pass wrote, with its --output $1.
counts() {
    local kind file count line=
    for kind in caught missed timeout unviable; do
        file="$1/mutants.out/$kind.txt"
        count=0
        if [[ -f $file ]]; then count=$(( $(wc -l < "$file") )); fi
        line+="${line:+, }$kind $count"
    done
    echo "$line"
}
# The regex that picks exactly the mutants the file $1 lists, for cargo-mutants' -F.
exactly() {
    python3 -c 'import re, sys
print("|".join("^" + re.escape(line.strip()) + "$" for line in open(sys.argv[1]) if line.strip()))' "$1"
}
# pass LINE OUTPUT ARG... runs a pass of cargo-mutants over $crate, with --output OUTPUT and the
# ARGs, and prints its line, LINE and the counts of its report, if it finished, or else its status:
# one that failed may have left no report, as cargo-mutants moves an earlier run's aside only once
# it has found the mutants, or one of some of them. It succeeds if the pass finished.
pass() {
    local line=$1 output=$2 ended=0
    shift 2
    cargo mutants -p "$crate" --test-tool nextest --jobs "$jobs" --output "$output" "$@" || ended=$?
    if ! finished "$ended"; then
        echo "$line: failed with status $ended"
        return 1
    fi
    echo "$line: $(counts "$output")"
}
listed=()
if [[ -n ${FXDAX_MUTANTS_LIST:-} ]]; then
    [[ -s $FXDAX_MUTANTS_LIST ]] || { echo "FXDAX_MUTANTS_LIST names no list" >&2; exit 2; }
    listed=(-F "$(exactly "$FXDAX_MUTANTS_LIST")")
fi
cd "$root"
status=0
for crate in "${crates[@]}"; do
    mkdir -p "$out/$crate"
    echo "== $crate" >&2
    # A sweep that failed has no missed of this run's to run again.
    if ! pass "$crate" "$out/$crate" ${shard[@]+"${shard[@]}"} ${listed[@]+"${listed[@]}"}; then
        status=1
        continue
    fi
    if grep -q '^shuttle' "crates/$crate/Cargo.toml" &&
        [[ -s "$out/$crate/mutants.out/missed.txt" ]]; then
        echo "== $crate, the missed under Shuttle" >&2
        # Exactly the sweep's missed (--iterate would also run every mutant of the crate a shard
        # never tried), into a report of their own: in the sweep's directory, cargo-mutants would
        # move the sweep's report aside for this one's.
        missed=$(exactly "$out/$crate/mutants.out/missed.txt")
        pass "$crate, the missed under Shuttle" "$out/$crate/shuttle" \
            --features shuttle -F "$missed" -- -E 'test(/shuttle/) | binary(shuttle)' || status=1
    fi
done
exit "$status"
