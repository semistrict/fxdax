#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Measure ranged GETs from object storage on an x86-64 Linux host of one's own, over ssh
# (FXDAX_HOST, ubu1 by default; scripts/lib/host.sh), as a measurement, alone there and on CPUs of
# its own (fxjob --bench) (TASK-28.11; doc-7, decision 9): fxdax-gets puts an object in the
# bucket, GETs parts of it by part size and GETs in flight, and deletes it
# (scripts/lib/gets-linux-host.sh). The table, the host and the source measured go to
# docs/measurements/HOST-gets-<time>-<mark>, the run's mark, unless named, with what the run was
# for (note.md).
#
#   scripts/gets-host.sh [RESULTS]
#   scripts/gets-host.sh resume RUN RESULTS | cancel RUN
#
# FXDAX_GETS_URL names the bucket and prefix the run puts its object under, as an object store's
# URL the host reaches (s3://bucket/prefix, with FXDAX_S3_* for an S3 server's address and keys;
# or file:///path/, a directory there, which measures that disk); FXDAX_GETS_MIB is the object's
# size (default 512) and FXDAX_GETS_SECS the seconds each measurement takes (default 10).
# FXDAX_BENCH_NOTE says what the run is for. Every other FXDAX_* and PJDFSTEST_* variable set here
# reaches the host too (scripts/lib/host.sh).
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$repo/scripts/lib/host.sh"
url=${FXDAX_GETS_URL:?FXDAX_GETS_URL names the bucket, as s3://bucket/prefix}
mib=${FXDAX_GETS_MIB:-512}
secs=${FXDAX_GETS_SECS:-10}
[[ "$url" =~ ^[a-z0-9]+://[A-Za-z0-9._/-]*$ ]] ||
    { echo 'FXDAX_GETS_URL is a URL, as s3://bucket/prefix' >&2; exit 2; }
[[ "$mib" =~ ^[1-9][0-9]*$ && "$secs" =~ ^[1-9][0-9]*$ ]] || { echo 'FXDAX_GETS_MIB and FXDAX_GETS_SECS are counts' >&2; exit 2; }
limit=2h
host_bench=1

# finish STATUS writes what the run was for, shows the table, and returns STATUS.
finish() {
    {
        echo "${FXDAX_BENCH_NOTE:-Ranged GETs from object storage (TASK-28.11).}"
        echo
        echo "Host: $host. Bucket: $url."
    } > "$results/note.md"
    if [[ -f $results/gets.md ]]; then cat "$results/gets.md" >&2; fi
    echo "Results: $results" >&2
    return "$1"
}
host_after=finish
host_actions "$@"
(( $# <= 1 )) || { echo "Usage: $0 [RESULTS] | resume RUN RESULTS | cancel RUN" >&2; exit 2; }
results=${1:-$repo/docs/measurements/$(host_label)-gets-$(date -u +%Y-%m-%dT%H%MZ)-$host_mark}

status=0
host_start
echo "Results: $results" >&2
host_job gets-linux-host.sh "${url%/}/$host_run" "$mib" "$secs" > "$results/remote.log" 2>&1 ||
    status=$?
if [[ $host_state == ended ]]; then
    host_fetch || status=$?
fi
finish "$status"
