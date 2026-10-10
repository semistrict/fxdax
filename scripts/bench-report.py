#!/usr/bin/env python3
"""Compares the filesystems the benchmark measured, as a Markdown table.

Reads the JSON each benchmark case wrote to a results directory (see scripts/bench.sh) and prints,
for each workload, each filesystem's median with its spread (minimum to maximum), and how many
times slower than ext4 on virtio-blk the others are: their median time over ext4's, or ext4's
median rate over theirs. Ratios over 2 are in bold. libkrun's own virtio-fs device, passing a host
directory through, shows what the transport fxdax shares costs.

Usage: scripts/bench-report.py RESULTS_DIR
"""

import json
import statistics
import sys
from pathlib import Path

BASELINE = "ext4-virtio-blk"
ORDER = [BASELINE, "fxdax-dax", "fxdax-nodax", "virtiofs-passthrough"]
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
    # The backends measured, in order; ratios only with ext4 to compare with.
    backends = [backend for backend in ORDER if backend in cases]
    others = backends[1:] if BASELINE in cases else []
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
    header = ["Workload", "Unit"] + backends + [f"{b} vs ext4" for b in others]
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
            if medians[backend] is None or medians[BASELINE] is None:
                row.append("–")
                continue
            if result["higher_is_better"]:
                ratio = medians[BASELINE] / medians[backend]
            else:
                ratio = medians[backend] / medians[BASELINE]
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
            print(f"- {workload} on {backend}: {ratio:.2f}×")
    else:
        print(f"Every workload is within {LIMIT:g}× of ext4.")


if __name__ == "__main__":
    main()
