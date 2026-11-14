#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Measure fxdax-gc's sweeps over a bucket of files of many cells (TASK-395.5): how long a sweep
# takes, and the most memory it holds, by how many passes it runs at once. It builds fxdax-gc and
# fxdax-gc-fill (release, extent_remote alone), fills WORK/bucket-SIDE once with FXDAX_GC_TENANTS
# tenants (100) of FXDAX_GC_CELLS cells (100) of a branch of FXDAX_GC_SEGMENTS segments (95), the
# largest cell's FXDAX_GC_LARGEST (950): about 1M objects in 10k cells by default, some 4 GB of
# files at most; then, for each count of FXDAX_GC_AT_ONCE ("1 4 16 64"), sweeps a copy of it
# (hard links on Linux, clones on macOS) FXDAX_GC_SWEEPS times (2, or 1): the first deleting and
# compacting half of every branch, the second deleting what the first compacted, and little else.
#
# Each SIDE=TREE is a tree whose fxdax-gc and fxdax-gc-fill are built (in WORK/target-SIDE) and
# measured, each with a bucket its own fill made, for an A/B of commits (TASK-395.17); with none,
# this tree's, as side `this`. The sweeps go in FXDAX_GC_ROUNDS rounds (1), each of every count on
# every side, the sides in turn first (A B, then B A), so that what drifts on the host moves all
# alike. Each sweep's metrics (fxdax-gc --metrics) and log go to
# RESULTS/sweep-SIDE-COUNT-rROUND-SWEEP.{prom,log}, the buckets' shapes to RESULTS/fill-SIDE.txt,
# a table of the sweeps to RESULTS/gc-sweep.md, and, with more than a round, each side's and
# count's median, least and most seconds a sweep. On ubu1, scripts/gc-sweep-host.sh runs it, as a
# measurement.
#
#   scripts/gc-sweep.sh WORK RESULTS [SIDE=TREE...]
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
(( $# >= 2 )) || { echo "Usage: $0 WORK RESULTS [SIDE=TREE...]" >&2; exit 2; }
mkdir -p "$1" "$2"
work=$(cd "$1" && pwd)
results=$(cd "$2" && pwd)
shift 2
tenants=${FXDAX_GC_TENANTS:-100}
cells=${FXDAX_GC_CELLS:-100}
segments=${FXDAX_GC_SEGMENTS:-95}
largest=${FXDAX_GC_LARGEST:-950}
at_once=${FXDAX_GC_AT_ONCE:-1 4 16 64}
sweeps=${FXDAX_GC_SWEEPS:-2}
rounds=${FXDAX_GC_ROUNDS:-1}
for count in "$tenants" "$cells" "$segments" "$largest" "$rounds" $at_once; do
    [[ $count =~ ^[1-9][0-9]*$ ]] || { echo "$count: not a count" >&2; exit 2; }
done
[[ $sweeps == 1 || $sweeps == 2 ]] || { echo "FXDAX_GC_SWEEPS is 1 or 2" >&2; exit 2; }
case $(uname) in
    Linux) copy=(cp -al) ;;
    Darwin) copy=(cp -Rc) ;;
    *) echo "$(uname): no way to copy a bucket cheaply" >&2; exit 2 ;;
esac

# The sides, and the directory each's binaries are in.
sides=()
bins=()
if (( $# == 0 )); then
    echo "== building fxdax-gc and fxdax-gc-fill" >&2
    cargo build --release -q --manifest-path "$repo/Cargo.toml" -p extent_remote \
        --bin fxdax-gc --bin fxdax-gc-fill
    sides=(this)
    bins=("${CARGO_TARGET_DIR:-$repo/target}/release")
fi
for pair in "$@"; do
    side=${pair%%=*}
    [[ $pair == *=* && $side =~ ^[a-z0-9][a-z0-9-]*$ ]] ||
        { echo "$pair: SIDE=TREE, SIDE in lower-case letters, digits and -" >&2; exit 2; }
    tree=$(cd "${pair#*=}" && pwd)
    echo "== $side: building fxdax-gc and fxdax-gc-fill of $tree" >&2
    # A target of its own, as two trees sharing one would link each other's crates.
    CARGO_TARGET_DIR=$work/target-$side cargo build --release -q \
        --manifest-path "$tree/Cargo.toml" -p extent_remote --bin fxdax-gc --bin fxdax-gc-fill
    sides+=("$side")
    bins+=("$work/target-$side/release")
done

# Each side's bucket, once for its shape.
shape="$tenants $cells $segments $largest"
for i in "${!sides[@]}"; do
    side=${sides[i]} bucket=$work/bucket-${sides[i]}
    if [[ ! -f $bucket.shape || $(cat "$bucket.shape") != "$shape" ]]; then
        rm -f "$bucket.shape"
        if [[ -e $bucket ]]; then rm -r -- "$bucket"; fi
        echo "== $side: filling $bucket: $tenants tenants of $cells cells of $segments segments" >&2
        started=$SECONDS
        "${bins[i]}/fxdax-gc-fill" "$bucket" "$tenants" "$cells" "$segments" "$largest" \
            > "$bucket.fill"
        sync
        echo "filled in $((SECONDS - started)) s" >&2
        echo "$shape" > "$bucket.shape"
    fi
    { echo "shape $shape"; cat "$bucket.fill"; } > "$results/fill-$side.txt"
done

# metric FILE NAME prints the value of the sample NAME (with its labels) in FILE.
metric() {
    awk -v name="$2" '$1 == name { print $2 }' "$1"
}

table=$results/gc-sweep.md
times=$results/times.tsv
{
    echo "Bucket: $(cat "$work/bucket-${sides[0]}.fill") (tenants $tenants, cells $cells, segments $segments, largest $largest)."
    echo
    echo "| side | at once | round | sweep | seconds | cells | deleted | compacted | requests | peak MiB |"
    echo "|---|---|---|---|---|---|---|---|---|---|"
} > "$table"
: > "$times"
for (( round = 1; round <= rounds; round++ )); do
    # The sides' order this round: as given, then turned, and so on.
    order=("${!sides[@]}")
    if (( round % 2 == 0 )); then
        order=()
        for (( i = ${#sides[@]} - 1; i >= 0; i-- )); do order+=("$i"); done
    fi
    for n in $at_once; do
        for i in "${order[@]}"; do
            side=${sides[i]}
            swept=$work/sweep-$side-$n
            if [[ -e $swept ]]; then rm -r -- "$swept"; fi
            "${copy[@]}" "$work/bucket-$side" "$swept"
            for (( sweep = 1; sweep <= sweeps; sweep++ )); do
                echo "== $side, $n at once, round $round, sweep $sweep" >&2
                name=$results/sweep-$side-$n-r$round-$sweep
                "${bins[i]}/fxdax-gc" "file://$swept/" --grace 0 --at-once "$n" \
                    --metrics "$name.prom" 2> "$name.log"
                tail -n 2 "$name.log" >&2
                seconds=$(metric "$name.prom" fxdax_gc_sweep_seconds)
                requests=$(awk '$1 ~ /^fxdax_gc_sweep_requests/ { sum += $2 } END { print sum }' \
                    "$name.prom")
                printf '| %s | %s | %s | %s | %.1f | %s | %s | %s | %s | %.0f |\n' \
                    "$side" "$n" "$round" "$sweep" "$seconds" \
                    "$(metric "$name.prom" 'fxdax_gc_sweep_cells{outcome="passed"}')" \
                    "$(metric "$name.prom" 'fxdax_gc_sweep_objects{action="deleted"}')" \
                    "$(metric "$name.prom" 'fxdax_gc_sweep_objects{action="compacted"}')" \
                    "$requests" \
                    "$(awk '$1 == "fxdax_gc_peak_resident_bytes" { print $2 / 1048576 }' \
                        "$name.prom")" \
                    >> "$table"
                printf '%s\t%s\t%s\t%s\n' "$side" "$n" "$sweep" "$seconds" >> "$times"
            done
            rm -r -- "$swept"
        done
    done
done

# Each side's, count's and sweep's seconds over the rounds: the median, the least and the most.
if (( rounds > 1 )); then
    {
        echo
        echo "| side | at once | sweep | rounds | median s | least s | most s |"
        echo "|---|---|---|---|---|---|---|"
        for side in "${sides[@]}"; do
            for n in $at_once; do
                for (( sweep = 1; sweep <= sweeps; sweep++ )); do
                    awk -F '\t' -v side="$side" -v n="$n" -v sweep="$sweep" \
                        '$1 == side && $2 == n && $3 == sweep { print $4 }' "$times" |
                        sort -g |
                        awk -v side="$side" -v n="$n" -v sweep="$sweep" '
                            { s[NR] = $1 }
                            END {
                                m = NR % 2 ? s[(NR + 1) / 2] : (s[NR / 2] + s[NR / 2 + 1]) / 2
                                printf "| %s | %s | %s | %d | %.1f | %.1f | %.1f |\n",
                                    side, n, sweep, NR, m, s[1], s[NR]
                            }'
                done
            done
        done
    } >> "$table"
fi
cat "$table" >&2
