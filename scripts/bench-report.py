#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Compares the filesystems the benchmark measured, as a Markdown table.

Reads the JSON each benchmark case wrote to a results directory (see scripts/bench.sh) and prints,
for each workload, each filesystem's median with its spread (minimum to maximum), and how many
times slower than its baseline the others are: their median time over the baseline's, or the
baseline's median rate over theirs. The guest kernel's Fxfs compares with ext4 with DAX on pmem,
or ext4 on virtio-blk, booted on the same kernel; a warm fork with a cold one. Ratios over 2 are in
bold. Runs before fxdax left libkrun (TASK-40) measured the default kernel's ext4 on virtio-blk
and libkrun's virtio-fs device too, which the others compared with: the report still reads them.

When the extent daemon kept stores in a bucket (FXDAX_EXTENTD_REMOTE), a section after the table
counts its requests by kind, with their estimated cost, and the bytes it uploaded and downloaded,
from the stats it kept in logs/extentd-stats.txt (extent_remote::store::Stats::line).

Usage: scripts/bench-report.py RESULTS_DIR
"""

import json
import statistics
import sys
from pathlib import Path

BASELINE = "ext4-virtio-blk"
ORDER = [
    BASELINE,
    "fxdax-dax",
    "fxdax-nodax",
    "virtiofs-passthrough",
    "ext4-guest-kernel",
    "ext4-dax-pmem",
    "fxfs-guest",
    "fxfs-guest-dax",
    "fxfs-guest-dax-fork-cold",
    "fxfs-guest-dax-fork-warm",
]
# What a filesystem compares with, unless ext4 on virtio-blk: the guest kernel's Fxfs with ext4 with
# DAX on writable pmem, booted on the same kernel, fxdax's guest kernel: storage as memory; or, if
# that wasn't measured, with ext4 on the same kernel.
BASELINES = {
    "ext4-dax-pmem": ["ext4-guest-kernel"],
    "fxfs-guest": ["ext4-dax-pmem", "ext4-guest-kernel"],
    "fxfs-guest-dax": ["ext4-dax-pmem", "ext4-guest-kernel"],
    # A fork booted with its base's data in the extent daemon's memory, against one without.
    "fxfs-guest-dax-fork-warm": ["fxfs-guest-dax-fork-cold"],
    "fxfs-guest-dax-fork-cold": [],
}


def baseline_of(backend, cases):
    """The first of `backend`'s baselines `cases` measured, if any."""
    return next((base for base in BASELINES.get(backend, [BASELINE]) if base in cases), None)
LIMIT = 2.0

# Object storage's prices, in dollars per 1,000 requests: GCS Standard storage in one region, flat
# namespace, as https://cloud.google.com/storage/pricing gave them on 2026-10-17 (S3 Standard's
# are the same). Class A: writes and listings; class B: reads; deletes are free.
PRICES = {"A": 0.005, "B": 0.0004, "free": 0.0}
CLASSES = {
    "get": "B",
    "get-range": "B",
    "head": "B",
    "list": "A",
    "create": "A",
    "update": "A",
    "overwrite": "A",
    "delete": "free",
}
UPLOADS = ["segments", "extents", "checkpoints", "moves", "other"]


def object_store_stats(directory):
    """What the extent daemon did with object storage, by name, if it kept stores there: its
    stats file (FXDAX_EXTENTD_STATS, which scripts/test-e2e.sh sets), one line of `NAME=COUNT`
    after `object-store:`."""
    path = Path(directory) / "logs" / "extentd-stats.txt"
    if not path.exists():
        return None
    fields = path.read_text().split("object-store:", 1)[1].split()
    return {name: int(value) for name, value in (field.split("=", 1) for field in fields)}


def size(count):
    for unit, scale in (("GiB", 1 << 30), ("MiB", 1 << 20), ("KiB", 1 << 10)):
        if count >= scale:
            return f"{count / scale:.1f} {unit}"
    return f"{count} B"


def object_storage(stats):
    """The section of the report on object storage, of `stats`, as lines."""
    lines = [
        "Object storage: the extent daemon's requests to its bucket, for the cases that kept their",
        "stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B",
        "$0.0004 per 1,000, deletes free).",
        "",
        "| Request | Count | Class | Cost |",
        "|---|---|---|---|",
    ]
    total_count, total_cost = 0, 0.0
    for request, kind in CLASSES.items():
        count = stats.get(request, 0)
        cost = count * PRICES[kind] / 1000
        total_count += count
        total_cost += cost
        lines.append(f"| {request} | {count:,} | {kind} | ${cost:.6f} |")
    lines.append(f"| all | {total_count:,} | | ${total_cost:.6f} |")
    lines.append("")
    uploaded = sum(stats.get(f"up-{kind}", 0) for kind in UPLOADS)
    by_kind = ", ".join(f"{kind} {size(stats.get(f'up-{kind}', 0))}" for kind in UPLOADS)
    lines.append(f"Uploaded {size(uploaded)} ({by_kind}); downloaded {size(stats.get('down', 0))}.")
    return lines


def load(directory):
    cases = {}
    for path in sorted(Path(directory).glob("bench-*.json")):
        report = json.loads(path.read_text())
        cases[report["backend"]] = report
    if not cases:
        sys.exit(f"{directory}: no results")
    return cases


def fmt(value):
    if value >= 100:
        return f"{value:,.0f}"
    if value >= 10:
        return f"{value:.1f}"
    return f"{value:.2f}"


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    cases = load(sys.argv[1])
    # The backends measured, in order; ratios only with their ext4 to compare with.
    backends = [backend for backend in ORDER if backend in cases]
    others = [b for b in backends if b != BASELINE and baseline_of(b, cases)]
    by_workload = {
        backend: {result["workload"]: result for result in report["results"]}
        for backend, report in cases.items()
    }
    baseline = cases[backends[0]]
    iterations = {backend: cases[backend]["iterations"] for backend in backends}
    print(
        f"Host: {baseline['host_os']}. VMM: {baseline.get('vmm', 'libkrun')}. Iterations: "
        + ", ".join(f"{backend} {n}" for backend, n in iterations.items())
        + ".\n"
    )
    header = ["Workload", "Unit"] + backends + [f"{b} vs {baseline_of(b, cases)}" for b in others]
    print("| " + " | ".join(header) + " |")
    print("|" + "---|" * len(header))
    over = []
    for result in baseline["results"]:
        workload = result["workload"]
        row = [workload, result["unit"]]
        medians = {}
        for backend in backends:
            values = by_workload[backend].get(workload, {}).get("values")
            if not values:
                row.append("failed")
                medians[backend] = None
                continue
            medians[backend] = statistics.median(values)
            row.append(f"{fmt(medians[backend])} ({fmt(min(values))}–{fmt(max(values))})")
        for backend in others:
            base = medians[baseline_of(backend, cases)]
            if medians[backend] is None or base is None:
                row.append("–")
                continue
            if result["higher_is_better"]:
                ratio = base / medians[backend]
            else:
                ratio = medians[backend] / base
            cell = f"{ratio:.2f}×"
            if ratio > LIMIT:
                cell = f"**{cell}**"
                over.append((workload, backend, ratio))
            row.append(cell)
        print("| " + " | ".join(row) + " |")
    print()
    for backend in backends:
        for workload, count in cases[backend].get("failures", {}).items():
            print(f"- {workload} failed in {count} of {backend}'s iterations, which have no measurement.")
        for name, values in cases[backend].get("observations", {}).items():
            print(
                f"- {backend}: {name} {fmt(statistics.median(values))} median "
                f"({fmt(min(values))}–{fmt(max(values))})"
            )
    print()
    stats = object_store_stats(sys.argv[1])
    if stats is not None:
        print("\n".join(object_storage(stats)))
        print()
    if not others:
        return
    if over:
        print(f"Over {LIMIT:g}× slower than ext4:")
        for workload, backend, ratio in over:
            print(f"- {workload} on {backend}: {ratio:.2f}× {baseline_of(backend, cases)}'s")
    else:
        print(f"Every workload is within {LIMIT:g}× of ext4.")


if __name__ == "__main__":
    main()
