#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Measure fxdax-gc's sweeps over a bucket of files of many cells (TASK-395.5): how long a sweep
# takes, and the most memory it holds, by how many passes it runs at once. It builds fxdax-gc and
# fxdax-gc-fill (release, extent_remote alone), fills WORK/bucket once with FXDAX_GC_TENANTS
# tenants (100) of FXDAX_GC_CELLS cells (100) of a branch of FXDAX_GC_SEGMENTS segments (95), the
# largest cell's FXDAX_GC_LARGEST (950): about 1M objects in 10k cells by default, some 4 GB of
# files at most; then, for each count of FXDAX_GC_AT_ONCE ("1 4 16 64"), sweeps a copy of it
# (hard links on Linux, clones on macOS) twice: the first deleting and compacting half of every
# branch, the second deleting what the first compacted, and little else. Each sweep's metrics
# (fxdax-gc --metrics) and log go to RESULTS/sweep-N-{1,2}.{prom,log}, the bucket's shape to
# RESULTS/fill.txt, and a table of them to RESULTS/gc-sweep.md. On ubu1, scripts/gc-sweep-host.sh
# runs it, as a measurement.
#
#   scripts/gc-sweep.sh WORK RESULTS
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
(( $# == 2 )) || { echo "Usage: $0 WORK RESULTS" >&2; exit 2; }
mkdir -p "$1" "$2"
work=$(cd "$1" && pwd)
results=$(cd "$2" && pwd)
tenants=${FXDAX_GC_TENANTS:-100}
cells=${FXDAX_GC_CELLS:-100}
segments=${FXDAX_GC_SEGMENTS:-95}
largest=${FXDAX_GC_LARGEST:-950}
at_once=${FXDAX_GC_AT_ONCE:-1 4 16 64}
for count in "$tenants" "$cells" "$segments" "$largest" $at_once; do
    [[ $count =~ ^[1-9][0-9]*$ ]] || { echo "$count: not a count" >&2; exit 2; }
done
case $(uname) in
    Linux) copy=(cp -al) ;;
    Darwin) copy=(cp -Rc) ;;
    *) echo "$(uname): no way to copy a bucket cheaply" >&2; exit 2 ;;
esac

echo "== building fxdax-gc and fxdax-gc-fill" >&2
cargo build --release -q --manifest-path "$repo/Cargo.toml" -p extent_remote \
    --bin fxdax-gc --bin fxdax-gc-fill
bin=${CARGO_TARGET_DIR:-$repo/target}/release

# The bucket, once for its shape.
shape="$tenants $cells $segments $largest"
bucket=$work/bucket
if [[ ! -f $work/bucket.shape || $(cat "$work/bucket.shape") != "$shape" ]]; then
    rm -f "$work/bucket.shape"
    if [[ -e $bucket ]]; then rm -r -- "$bucket"; fi
    echo "== filling $bucket: $tenants tenants of $cells cells of $segments segments" >&2
    started=$SECONDS
    "$bin/fxdax-gc-fill" "$bucket" "$tenants" "$cells" "$segments" "$largest" > "$work/bucket.fill"
    sync
    echo "filled in $((SECONDS - started)) s" >&2
    echo "$shape" > "$work/bucket.shape"
fi
{ echo "shape $shape"; cat "$work/bucket.fill"; } > "$results/fill.txt"

# metric FILE NAME prints the value of the sample NAME (with its labels) in FILE.
metric() {
    awk -v name="$2" '$1 == name { print $2 }' "$1"
}

table=$results/gc-sweep.md
{
    echo "Bucket: $(cat "$work/bucket.fill") (tenants $tenants, cells $cells, segments $segments, largest $largest)."
    echo
    echo "| at once | sweep | seconds | cells | deleted | compacted | requests | peak MiB |"
    echo "|---|---|---|---|---|---|---|---|"
} > "$table"
for n in $at_once; do
    swept=$work/sweep-$n
    if [[ -e $swept ]]; then rm -r -- "$swept"; fi
    "${copy[@]}" "$bucket" "$swept"
    for round in 1 2; do
        echo "== $n at once, sweep $round" >&2
        prom=$results/sweep-$n-$round.prom
        "$bin/fxdax-gc" "file://$swept/" --grace 0 --at-once "$n" --metrics "$prom" \
            2> "$results/sweep-$n-$round.log"
        tail -n 2 "$results/sweep-$n-$round.log" >&2
        requests=$(awk '$1 ~ /^fxdax_gc_sweep_requests/ { sum += $2 } END { print sum }' "$prom")
        printf '| %s | %s | %.1f | %s | %s | %s | %s | %.0f |\n' "$n" "$round" \
            "$(metric "$prom" fxdax_gc_sweep_seconds)" \
            "$(metric "$prom" 'fxdax_gc_sweep_cells{outcome="passed"}')" \
            "$(metric "$prom" 'fxdax_gc_sweep_objects{action="deleted"}')" \
            "$(metric "$prom" 'fxdax_gc_sweep_objects{action="compacted"}')" \
            "$requests" \
            "$(awk '$1 == "fxdax_gc_peak_resident_bytes" { print $2 / 1048576 }' "$prom")" \
            >> "$table"
    done
    rm -r -- "$swept"
done
cat "$table" >&2
