#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Compares the filesystems the benchmark measured, as a Markdown table.

Reads the JSON each benchmark case wrote to a results directory (see scripts/bench.sh) and prints,
for each workload, each filesystem's median with its spread (minimum to maximum), and how many
times slower than its baseline the others are: their median time over the baseline's, or the
baseline's median rate over theirs. The guest kernel's Fxfs compares with ext4 with DAX on pmem,
or ext4 on virtio-blk, booted on the same kernel; a warm fork with a cold one; a writer with
followers with one alone (scripts/bench_results.py, which scripts/bench-history.py shares). Ratios
over 2 are in bold. Runs before fxdax left libkrun (TASK-40) measured the default kernel's ext4 on
virtio-blk and libkrun's virtio-fs device too, which the others compared with: the report still
reads them.

When the extent daemon kept stores in a bucket (FXDAX_EXTENTD_REMOTE), a section after the table
counts its requests by kind, with their estimated cost, and the bytes it uploaded and downloaded,
from the stats it kept in logs/extentd-stats.txt (extent_remote::store::Stats::line). Another
says what the VM processes' requests took the daemon, by kind, and its commits, by phase.

Usage: scripts/bench-report.py RESULTS_DIR
"""

import importlib.util
import statistics
import sys
from pathlib import Path

# By its path, which needs no module path naming this directory.
spec = importlib.util.spec_from_file_location(
    "bench", Path(__file__).resolve().with_name("bench_results.py")
)
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


def baseline_of(backend, cases):
    """The first of `backend`'s baselines `cases` measured, if any."""
    return next((base for base in bench.baselines(backend) if base in cases), None)


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


def daemon_stats(directory, kind):
    """The extent daemon's stats of `kind`, by name, if it kept them: its stats file
    (FXDAX_EXTENTD_STATS, which scripts/test-e2e.sh sets), a line of `NAME=COUNT` after `KIND:`
    for each kind: object-store (what it did with object storage, if it kept stores there),
    followers (what the stores' followers were), daemon (what serving the stores cost it) and
    requests (the VM processes' requests it served, by kind)."""
    path = Path(directory) / "logs" / "extentd-stats.txt"
    if not path.exists():
        return None
    for line in path.read_text().splitlines():
        if line.startswith(f"{kind}:"):
            fields = line.split(":", 1)[1].split()
            return {name: int(value) for name, value in (field.split("=", 1) for field in fields)}
    return None


def object_store_stats(directory):
    """What the extent daemon did with object storage, by name, if it kept stores there."""
    return daemon_stats(directory, "object-store")


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


def followers(stats):
    """The section of the report on followers (backlog doc-8), of `stats`, as lines: if any
    followed."""
    if not stats.get("max-views", 0):
        return []
    return [
        f"Followers: at most {stats['max-views']} at once, at most {stats['max-behind']} commits and "
        f"{stats['max-stale-ms']:,} ms behind their writer, keeping at most "
        f"{size(stats['max-kept'])} of its frees.",
    ]


def served(requests, daemon):
    """The section of the report on the requests the extent daemon served, of `requests` (a count
    and nanoseconds by kind) and `daemon` (what serving them cost, `fxdax::stats::Stats`), as
    lines: if it served any."""
    kinds = [name for name in requests if not name.endswith("-ns")]
    if not kinds:
        return []
    lines = [
        "Extent daemon: the VM processes' requests, as it served them, from their dispatch to their",
        "reply.",
        "",
        "| Request | Count | Each |",
        "|---|---|---|",
    ]
    for kind in kinds:
        count = requests[kind]
        lines.append(f"| {kind} | {count:,} | {requests[f'{kind}-ns'] / count / 1e3:,.1f} µs |")
    commits = daemon.get("commits", 0)
    if commits:

        def each(name):
            return daemon[name] / commits / 1e3

        lines += [
            "",
            f"Commits: {commits:,}, {each('commit-ns'):,.1f} µs each: writing the open extents' "
            f"{daemon['commit-written-bytes'] / commits / 1024:,.1f} KiB "
            f"{each('commit-write-ns'):,.1f} µs, committing the store "
            f"{each('commit-store-ns'):,.1f} µs, sealing the windows' mappings "
            f"{each('commit-seal-ns'):,.1f} µs, waiting for object storage "
            f"{each('commit-wait-ns'):,.1f} µs.",
        ]
    closes = daemon.get("segment-closes", 0)
    if closes:
        lines += [
            "",
            f"Segments closed and framed: {closes:,}, "
            f"{daemon['segment-close-ns'] / closes / 1e6:,.1f} ms each.",
        ]
    return lines


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    cases = bench.load(sys.argv[1]) or sys.exit(f"{sys.argv[1]}: no results")
    # The backends measured, in order; ratios only for those whose baseline was measured too.
    backends = bench.ordered(cases)
    others = [b for b in backends if baseline_of(b, cases)]
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
            row.append(
                f"{bench.fmt(medians[backend])} "
                f"({bench.fmt(min(values))}–{bench.fmt(max(values))})"
            )
        for backend in others:
            base = medians[baseline_of(backend, cases)]
            if medians[backend] is None or base is None:
                row.append("–")
                continue
            ratio = bench.ratio(medians[backend], base, result["higher_is_better"])
            if ratio > bench.LIMIT:
                over.append((workload, backend, ratio))
            row.append(bench.times(ratio))
        print("| " + " | ".join(row) + " |")
    print()
    for backend in backends:
        for workload, count in cases[backend].get("failures", {}).items():
            print(f"- {workload} failed in {count} of {backend}'s iterations, which have no measurement.")
        for name, values in cases[backend].get("observations", {}).items():
            print(
                f"- {backend}: {name} {bench.fmt(statistics.median(values))} median "
                f"({bench.fmt(min(values))}–{bench.fmt(max(values))})"
            )
    print()
    stats = object_store_stats(sys.argv[1])
    if stats is not None:
        print("\n".join(object_storage(stats)))
        print()
    following = followers(daemon_stats(sys.argv[1], "followers") or {})
    if following:
        print("\n".join(following))
        print()
    requests = daemon_stats(sys.argv[1], "requests") or {}
    serving = served(requests, daemon_stats(sys.argv[1], "daemon") or {})
    if serving:
        print("\n".join(serving))
        print()
    if not others:
        return
    if over:
        print(f"Over {bench.LIMIT:g}× slower than ext4:")
        for workload, backend, ratio in over:
            print(f"- {workload} on {backend}: {ratio:.2f}× {baseline_of(backend, cases)}'s")
    else:
        print(f"Every workload is within {bench.LIMIT:g}× of ext4.")


if __name__ == "__main__":
    main()
