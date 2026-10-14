#!/usr/bin/env python3
"""Compares the filesystems the benchmark measured, as a Markdown table.

Reads the JSON each benchmark case wrote to a results directory (see scripts/bench.sh) and prints,
for each workload, each filesystem's median with its spread (minimum to maximum), and how many
times slower than ext4 on virtio-blk the others are: their median time over ext4's, or ext4's
median rate over theirs. The guest kernel's Fxfs compares with ext4 booted on the same kernel. Ratios over 2 are in bold. libkrun's own virtio-fs device, passing a host
directory through, shows what the transport fxdax shares costs.

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
]
# What a filesystem compares with, unless ext4 on virtio-blk: the guest kernel's Fxfs with ext4 with
# DAX on writable pmem, booted on the same kernel, fxdax's guest kernel: storage as memory; or, if
# that wasn't measured, with ext4 on the same kernel.
BASELINES = {
    "ext4-dax-pmem": ["ext4-guest-kernel"],
    "fxfs-guest": ["ext4-dax-pmem", "ext4-guest-kernel"],
    "fxfs-guest-dax": ["ext4-dax-pmem", "ext4-guest-kernel"],
}


def baseline_of(backend, cases):
    """The first of `backend`'s baselines `cases` measured, if any."""
    return next((base for base in BASELINES.get(backend, [BASELINE]) if base in cases), None)
LIMIT = 2.0


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
        f"Host: {baseline['host_os']}. Iterations: "
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
