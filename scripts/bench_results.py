# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""The filesystem benchmark's results, as scripts/bench-report.py and scripts/bench-history.py read
them: which filesystems there are, the order they are shown in, what each compares with, and how
many times slower than that it was. Both load it by its path.
"""

import json
from pathlib import Path

BASELINE = "ext4-virtio-blk"
# The filesystems in the order they are shown in (`ordered`).
ORDER = [
    BASELINE,
    "fxdax-dax",
    "fxdax-nodax",
    "virtiofs-passthrough",
    "ext4-guest-kernel",
    "ext4-dax-pmem",
    "fxfs-guest",
    "fxfs-guest-dax",
    "fxfs-guest-dax-followed",
    "fxfs-guest-dax-followed-idle",
    "fxfs-guest-dax-fork-cold",
    "fxfs-guest-dax-fork-profiled",
    "fxfs-guest-dax-fork-warm",
]
# What a filesystem compares with, in order of preference: the first of them a run measured. Unless
# named here, ext4 on virtio-blk, which runs before fxdax left libkrun (TASK-40) compared every
# other filesystem with.
BASELINES = {
    BASELINE: [],
    # The guest kernel's Fxfs with ext4 with DAX on writable pmem, booted on the same kernel,
    # fxdax's guest kernel: storage as memory; or, if that wasn't measured, with ext4 on the same
    # kernel.
    "ext4-dax-pmem": ["ext4-guest-kernel"],
    "fxfs-guest": ["ext4-dax-pmem", "ext4-guest-kernel"],
    "fxfs-guest-dax": ["ext4-dax-pmem", "ext4-guest-kernel"],
    # A writer with followers attached, reading or not, against one alone (TASK-42.7).
    "fxfs-guest-dax-followed": ["fxfs-guest-dax"],
    "fxfs-guest-dax-followed-idle": ["fxfs-guest-dax"],
    # A fork booted with its base's data in the extent daemon's memory, against one without.
    "fxfs-guest-dax-fork-warm": ["fxfs-guest-dax-fork-cold"],
    "fxfs-guest-dax-fork-cold": [],
    # A cold fork with its base's boot profile read ahead, against one with the base's data in memory.
    "fxfs-guest-dax-fork-profiled": ["fxfs-guest-dax-fork-warm"],
}
# A filesystem more times slower than its baseline than this is in bold.
LIMIT = 2.0
# The workloads of each group the benchmark's guest records a failure of as a whole: an iteration
# whose build or pgbench failed has `FAILED build` or `FAILED pgbench` (test_fs_bench.rs), and none
# of the group's measurements from the one that failed on. Any other failure names its workload
# (setxattr, on a filesystem without extended attributes).
GROUPS = {
    "build": ["untar", "make", "build-total"],
    "pgbench": ["pgbench-init", "pgbench-tps"],
}


def baselines(backend):
    """What `backend` compares with, in order of preference."""
    return BASELINES.get(backend, [BASELINE])


def failed(workload, failures):
    """Whether `failures`, a case's iterations that failed by what the guest named, name `workload`
    or its group."""
    return any(workload in GROUPS.get(name, [name]) for name in failures)


def ordered(backends):
    """`backends` in ORDER's order, then those it does not name, by name: a filesystem a case
    measured is shown, even before it is named here."""
    named = [backend for backend in ORDER if backend in backends]
    return named + sorted(set(backends) - set(ORDER))


def load(directory):
    """The measurements of the run in `directory`, each case's (bench-CASE.json), by filesystem."""
    cases = {}
    for path in sorted(Path(directory).glob("bench-*.json")):
        report = json.loads(path.read_text())
        cases[report["backend"]] = report
    return cases


def ratio(value, base, higher_is_better):
    """How many times slower `value` is than `base`: its time over the baseline's, or the
    baseline's rate over its."""
    return base / value if higher_is_better else value / base


def times(value):
    """The ratio `value` as a table's cell, in bold over LIMIT."""
    cell = f"{value:.2f}×"
    return f"**{cell}**" if value > LIMIT else cell


def fmt(value):
    """A measurement as a table's cell."""
    if value >= 100:
        return f"{value:,.0f}"
    if value >= 10:
        return f"{value:.1f}"
    return f"{value:.2f}"
