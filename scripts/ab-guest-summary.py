#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Compares the sides of an A/B of guest cases (scripts/ab-guest-cases.sh), OUT its results: for
each case, each side's runs, by what the extent daemon of each said (its stats file's `daemon:`
line and its log) and the case's own `shared-disk:` lines: the slowest framing and how many took
past extentd's turn, the slowest making of segments framed durable (TASK-310), the commits'
durable lag (median, 99th percentile, longest, within 50 ms), what the daemon wrote, the
shared-disk case's seconds, and pgbench's tps. Each cell is the median of a side's runs and their
range. It prints a Markdown table per case.

Usage: scripts/ab-guest-summary.py OUT
"""

import re
import statistics
import sys
from pathlib import Path


def deepest(run, name):
    """The file `name` in `run`, the case's own if the case started a daemon of its own."""
    found = sorted(run.rglob(name), key=lambda path: len(path.parts))
    return found[-1] if found else None


def daemon_stats(run):
    path = deepest(run, "extentd-stats.txt")
    if path is None:
        return {}
    for line in path.read_text().splitlines():
        if line.startswith("daemon: "):
            fields = (field.split("=", 1) for field in line[len("daemon: "):].split())
            return {name: int(value) for name, value in fields if value.isdigit()}
    return {}


def slowest(stats, kind):
    """The phases of the slowest `kind` ("framing", "durable", "flush"), summed, in ms."""
    phases = [value for name, value in stats.items()
              if name.startswith(f"{kind}_slowest_") and name.endswith("_us")]
    return sum(phases) / 1000 if phases else None


def measures(run):
    """What a run of a case measured, by name."""
    stats = daemon_stats(run)
    got = {}
    got["slowest framing (ms)"] = slowest(stats, "framing")
    got["framings past 50 ms"] = stats.get("framing_slow")
    got["slowest making durable (ms)"] = slowest(stats, "durable")
    got["slowest flush (ms)"] = slowest(stats, "flush")
    count = stats.get("durable_lag_count")
    if count:
        got["durable lag within 50 ms (%)"] = 100 * stats["durable_lag_within_50ms"] / count
        got["durable lag p50 (ms)"] = stats.get("durable_lag_p50_ms")
        got["durable lag p99 (ms)"] = stats.get("durable_lag_p99_ms")
        got["durable lag max (ms)"] = stats["durable_lag_max_us"] / 1000
    if "io_write_bytes" in stats:
        got["extentd wrote (GiB)"] = stats["io_write_bytes"] / (1 << 30)
    texts = [path.read_text(errors="replace") for path in run.rglob("log.txt")]
    texts += [path.read_text(errors="replace") for path in run.rglob("stdout.txt")]
    text = "\n".join(texts)
    secs = re.search(r"shared-disk: vms=\S+ at_once=\S+ dax=\S+ secs=([0-9.]+)", text)
    if secs:
        got["shared-disk secs"] = float(secs.group(1))
    tps = [float(match) for match in re.findall(r"tps = ([0-9.]+)", text)]
    if tps:
        got["pgbench tps (mean of runs)"] = statistics.mean(tps)
    got["failed"] = 0 if passed(run) else 1
    return {name: value for name, value in got.items() if value is not None}


def passed(run):
    """Whether the run's case passed, as scripts/test-e2e.sh says: `OK - N/N passed`."""
    log = run / "run.log"
    said = log.read_text(errors="replace") if log.exists() else ""
    return re.search(r"^OK - (\d+)/\1 passed", said, re.M) is not None


def cell(values):
    if not values:
        return "-"
    fmt = (lambda v: f"{v:.0f}") if max(values) >= 100 else (lambda v: f"{v:.2f}")
    median = statistics.median(values)
    return f"{fmt(median)} ({fmt(min(values))}-{fmt(max(values))}, n={len(values)})"


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    out = Path(sys.argv[1])
    for case in sorted(path for path in out.iterdir() if path.is_dir()):
        runs = {}
        for run in sorted(path for path in case.iterdir() if path.is_dir()):
            side = run.name.rsplit("-", 1)[0]
            runs.setdefault(side, []).append(measures(run))
        sides = sorted(runs)
        names = []
        for side in sides:
            for got in runs[side]:
                names.extend(name for name in got if name not in names)
        print(f"### {case.name}\n")
        print("| | " + " | ".join(sides) + " |")
        print("|---|" + "---|" * len(sides))
        for name in names:
            cells = [cell([got[name] for got in runs[side] if name in got]) for side in sides]
            print(f"| {name} | " + " | ".join(cells) + " |")
        print()


if __name__ == "__main__":
    main()
