#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Gates a GCE run of the sandbox team's pgbench on its throughput (TASK-131.5).

Reads the pgbench cases' logs of scripts/test-gce.sh results directories (fxfs-guest-pgbench,
without a DAX window, fxfs-guest-pgbench-dax, with one, and pgbench-ext4-dax-pmem, ext4 with DAX on
virtio-pmem, a static DAX device: e2e/CASE/CASE/log.txt, or each round's, round-K/CASE/log.txt),
their pgbench runs' tps in order, RUNS a VM as the guest's output beside it says (stdout.txt's
`OK runs RUNS`), as many VMs as one daemon served (FXDAX_PGBENCH_VMS),
each directory's rounds after the last's; writes them, and the gates, as JSON; and fails unless
every gate passes:

- decay: each of the guest Fxfs's cases' last run of its first VM against its first run (the
  sandbox team saw ~21% lost over five), at least DECAY;
- dax: DAX against no DAX, each case's median run of its first VM, at least DAX;
- static-dax: DAX against ext4 with DAX on pmem, as `dax`: the gap to a static DAX device, at least
  STATIC_DAX, by default 0, reported and not gated;
- density: of one daemon serving many VMs, the first round that had more than one, its last VM's
  median run against its first's, at least DENSITY.

Each run's guest latencies (TASK-111.6), the histograms the guest's Fxfs keeps of its faults, seals,
evacuations and window MAPs (`NAME_hist` in /proc/fs/fxfs/extents, which the case's log has before
and after each run on its `fxfs stats:` lines), are reported as their percentiles, in us: the upper
bound of the bucket each falls in, bucket k's 1.024 us times 2 to the k, the last's none.

A gate whose cases didn't run is left out. The host's kind (`nested`, as GCE's n2 VMs are, or
`bare-metal`) is reported with the numbers, which differ much between the two (window remaps and
first-touch mappings cost more nested), never compared across.

Usage: scripts/pgbench-gate.py [--host nested|bare-metal] [--json PATH] RESULTS...

The thresholds: FXDAX_GATE_DECAY (default 0.9), FXDAX_GATE_DAX (0.95), FXDAX_GATE_STATIC_DAX (0),
FXDAX_GATE_DENSITY (0.9).
"""

import argparse
import json
import os
import re
import statistics
import sys
from pathlib import Path

# The cases, by the name the JSON gives them.
CASES = {
    "fxfs-guest-pgbench": "no-dax",
    "fxfs-guest-pgbench-dax": "dax",
    "pgbench-ext4-dax-pmem": "ext4-dax-pmem",
}
# Those of the guest's Fxfs, which its gates of decay and density hold to.
FXFS = ("no-dax", "dax")
# A run's result, as pgbench prints it.
TPS = re.compile(r"^tps = ([0-9.]+) \(without initial connection time\)")
# The runs a VM makes, as its guest says once they are done (test_fxfs_guest_pgbench.rs).
RUNS = re.compile(r"^OK runs ([0-9]+)$")
# The guest's Fxfs's stats, names and values, on one line, as the case logs them around each run.
STATS = re.compile(r"^fxfs stats: (.*)$")
# The percentiles of each run's latencies reported.
PERCENTILES = (50, 90, 99)
DEFAULTS = {"decay": 0.9, "dax": 0.95, "static-dax": 0.0, "density": 0.9}


def records(lines):
    """Each pgbench run's record in `lines`, a case's log, in order: its tps, and its guest's
    latencies' percentiles (`latencies`); and the runs a VM made, if the guest said."""
    found, stats, per_vm = [], [], None
    for line in lines:
        if match := STATS.match(line):
            stats.append(match.group(1).split())
        elif match := TPS.match(line):
            # The run's stats are the last before its tps, and those before them, before it.
            between = latencies(stats[-2], stats[-1]) if len(stats) > 1 else {}
            found.append((float(match.group(1)), between))
        elif match := RUNS.match(line.strip()):
            per_vm = int(match.group(1))
    return found, per_vm


def by_vm(values, per_vm):
    """`values`, a value each run, VM by VM: `per_vm` runs each, or all one VM's."""
    per_vm = per_vm or len(values)
    return [values[at:at + per_vm] for at in range(0, len(values), per_vm)]


def runs(lines):
    """The tps of each VM's pgbench runs in `lines`, a case's log, VM by VM, in order."""
    found, per_vm = records(lines)
    return by_vm([tps for tps, _ in found], per_vm)


def latencies(before, after):
    """The percentiles of each histogram's counts between the stats `before` and `after` (each a
    stats line's words, names and values in turn), by the name before its `_hist`: how many, and
    each percentile's bucket's upper bound in us."""
    pairs = [dict(zip(words[::2], words[1::2])) for words in (before, after)]
    found = {}
    for name, value in pairs[1].items():
        if not name.endswith("_hist") or name not in pairs[0]:
            continue
        counts = [int(now) - int(then)
                  for now, then in zip(value.split(","), pairs[0][name].split(","))]
        found[name.removesuffix("_hist")] = percentiles(counts)
    return found


def percentiles(counts):
    """How many `counts`, a histogram's buckets, count, and each of PERCENTILES' bucket's upper
    bound in us: none for the last bucket, which has none, nor for no count."""
    total, found = sum(counts), {}
    for percentile in PERCENTILES:
        bound = None
        if total:
            reached, at = 0, 0
            for at, count in enumerate(counts):
                reached += count
                if reached * 100 >= percentile * total:
                    break
            bound = None if at == len(counts) - 1 else round(1.024 * 2 ** at, 3)
        found[f"p{percentile}_us"] = bound
    return {"count": total, **found}


def logs(results, case):
    """The logs of `case` in `results`, a test-gce.sh results directory: each round's, in order."""
    root = Path(results) / "e2e" / case
    single = root / case / "log.txt"
    if single.exists():
        return [single]
    rounds = sorted(root.glob(f"round-*/{case}/log.txt"),
                    key=lambda path: int(path.parent.parent.name.split("-")[1]))
    return rounds


def said(log):
    """The lines of a case's `log`, its console, where pgbench says its tps, then of its guest's
    output beside it (stdout.txt), where the guest says how many runs a VM made."""
    lines = log.read_text(errors="replace").splitlines()
    stdout = log.with_name("stdout.txt")
    if stdout.exists():
        lines += stdout.read_text(errors="replace").splitlines()
    return lines


def measure(*results):
    """Each case's rounds in the `results` directories, by its JSON name: each round's VMs' runs'
    tps, each directory's rounds after the last's."""
    return measure_each(results, lambda found: [tps for tps, _ in found])


def measure_latencies(*results):
    """As `measure`, each run's guest latencies' percentiles (`latencies`): of the cases whose
    guests keep them."""
    measured = measure_each(results, lambda found: [between for _, between in found])
    return {name: rounds for name, rounds in measured.items()
            if any(run for vms in rounds for runs in vms for run in runs)}


def measure_each(results, values):
    """Each case's rounds in the `results` directories, by its JSON name: each round's VMs' runs'
    `values`, of its runs' records (`records`), each directory's rounds after the last's."""
    measured = {}
    for case, name in CASES.items():
        rounds = []
        for directory in results:
            for log in logs(directory, case):
                found, per_vm = records(said(log))
                rounds.append(by_vm(values(found), per_vm) if found else [])
        if any(rounds):
            measured[name] = [vms for vms in rounds if vms]
    return measured


def gates(measured, thresholds):
    """The gates of `measured`, as `measure` has it, against `thresholds`, by gate: each a dict of
    its name, the case, the value and its threshold, and whether it passed. Of each case's first
    round, but density's, of its first round of more than one VM."""
    found = []

    def gate(name, case, value):
        found.append({"gate": name, "case": case, "value": round(value, 4),
                      "threshold": thresholds[name], "passed": value >= thresholds[name]})

    def median_against(case, other):
        return statistics.median(first[case][0]) / statistics.median(first[other][0])

    first = {case: rounds[0] for case, rounds in measured.items()}
    for case in FXFS:
        if case not in measured:
            continue
        vms = first[case]
        gate("decay", case, vms[0][-1] / vms[0][0])
        dense = next((vms for vms in measured[case] if len(vms) > 1), None)
        if dense:
            gate("density", case, statistics.median(dense[-1]) / statistics.median(dense[0]))
    if "dax" in first and "no-dax" in first:
        gate("dax", "dax", median_against("dax", "no-dax"))
    if "dax" in first and "ext4-dax-pmem" in first:
        gate("static-dax", "dax", median_against("dax", "ext4-dax-pmem"))
    return found


def thresholds():
    """The gates' thresholds: the environment's FXDAX_GATE_<GATE>, or their defaults."""
    return {name: float(os.environ.get(f"FXDAX_GATE_{name.upper().replace('-', '_')}", default))
            for name, default in DEFAULTS.items()}


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--host", choices=["nested", "bare-metal"], default="nested")
    parser.add_argument("--json")
    parser.add_argument("results", nargs="+")
    args = parser.parse_args()
    measured = measure(*args.results)
    if not measured:
        raise SystemExit(f"no pgbench runs in {' '.join(args.results)}")
    report = {"host": args.host, "tps": measured, "gates": gates(measured, thresholds())}
    if latency := measure_latencies(*args.results):
        report["latency"] = latency
    text = json.dumps(report, indent=2)
    if args.json:
        Path(args.json).write_text(text + "\n")
    print(text)
    failed = [gate for gate in report["gates"] if not gate["passed"]]
    for gate in failed:
        print(f"failed: {gate['gate']} of {gate['case']}: {gate['value']} under "
              f"{gate['threshold']}", file=sys.stderr)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
