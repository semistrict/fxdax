# shellcheck shell=bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Sourced by the scripts whose runs load a Linux host for long (scripts/sandboxes.sh): what the
# host and the run's cgroup spend as the run goes, into a log that outlives a run that runs out of
# them. The sandboxes' long mode ran a CI runner out of memory and down, its artifacts with it, and
# on ubu1 ran past 2 hours unfinished with nothing kept to say why (TASK-330.5).

# resources_sample prints one sample: the time; the host's load, memory and CPU time by kind
# (/proc/stat's, in clock ticks); the run's cgroup's memory by kind, its memory events, its CPU
# time and throttling, and its CPU, memory and I/O pressure (PSI), whose `some` says how long
# something of it waited for each; and the host's ten largest processes and ten busiest, by their
# CPU time so far, others' among them, which a shared host's contention shows in.
resources_sample() {
    local cgroup=$1 file
    echo "== $(date -u +%FT%TZ) load $(cut -d' ' -f1-4 /proc/loadavg)"
    grep -E '^(MemFree|MemAvailable|Cached|Dirty|Writeback|AnonPages|Shmem|SwapFree):' \
        /proc/meminfo | tr -s ' ' | tr '\n' ' '
    echo
    sed -n 's/^cpu  /cpu /p' /proc/stat
    if [[ -n $cgroup ]]; then
        echo "cgroup $(cat "$cgroup/memory.current") bytes," \
            "peak $(cat "$cgroup/memory.peak" 2> /dev/null || echo -)"
        grep -E '^(anon|file|shmem|file_dirty|file_writeback|pgmajfault|workingset_refault_file) ' \
            "$cgroup/memory.stat" | tr '\n' ' '
        echo
        tr '\n' ' ' < "$cgroup/memory.events"
        echo
        tr '\n' ' ' < "$cgroup/cpu.stat"
        echo
        for file in cpu memory io; do
            echo "$file.pressure $(tr '\n' ' ' < "$cgroup/$file.pressure")"
        done
    fi
    ps -eo pid,rss,time,comm --sort=-rss --no-headers | sed -n 1,10p
    ps -eo pid,rss,time,comm --sort=-time --no-headers | sed -n 1,10p
}

# resources_watch LOG [EVERY] samples the host and this process's cgroup (resources_sample) every
# EVERY seconds (30), appending to LOG, in the background, until this shell exits; its pid is in
# resources_watcher.
resources_watch() {
    local log=$1 every=${2:-30} cgroup
    cgroup=/sys/fs/cgroup$(sed -n 's/^0:://p' /proc/self/cgroup)
    [[ -f $cgroup/memory.current ]] || cgroup=
    (
        while true; do
            resources_sample "$cgroup"
            sleep "$every"
        done
    ) >> "$log" 2>&1 &
    resources_watcher=$!
}
