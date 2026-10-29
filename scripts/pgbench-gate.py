#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Gates a GCE run of the sandbox team's pgbench on its throughput (TASK-131.5).

Reads the pgbench cases' logs of a scripts/test-gce.sh results directory (fxfs-guest-pgbench,
without a DAX window, and fxfs-guest-pgbench-dax, with one: e2e/CASE/CASE/log.txt, or each round's,
round-K/CASE/log.txt), their pgbench runs' tps in order, RUNS a VM, as many VMs as one daemon served
(FXDAX_PGBENCH_VMS); writes them, and the gates, as JSON; and fails unless every gate passes:

- decay: each case's last run of its first VM against its first run (the sandbox team saw ~21%
  lost over five), at least DECAY;
- dax: DAX against no DAX, each case's median run of its first VM, at least DAX;
- density: of one daemon serving many VMs, its last VM's median run against its first's, at least
  DENSITY.

A gate whose cases didn't run is left out. The host's kind (`nested`, as GCE's n2 VMs are, or
`bare-metal`) is reported with the numbers, which differ much between the two (window remaps and
first-touch mappings cost more nested), never compared across.

Usage: scripts/pgbench-gate.py [--host nested|bare-metal] [--json PATH] RESULTS

The thresholds: FXDAX_GATE_DECAY (default 0.9), FXDAX_GATE_DAX (0.95), FXDAX_GATE_DENSITY (0.9).
"""

import argparse
import json
import os
import re
import statistics
import sys
from pathlib import Path

# The cases, by the name the JSON gives them, and whether they map a DAX window.
CASES = {"fxfs-guest-pgbench": "no-dax", "fxfs-guest-pgbench-dax": "dax"}
# A run's result, as pgbench prints it.
TPS = re.compile(r"^tps = ([0-9.]+) \(without initial connection time\)")
# The runs a VM makes, as its guest says once they are done (test_fxfs_guest_pgbench.rs).
RUNS = re.compile(r"^OK runs ([0-9]+)$")
DEFAULTS = {"decay": 0.9, "dax": 0.95, "density": 0.9}


def runs(lines):
    """The tps of each VM's pgbench runs in `lines`, a case's log, VM by VM, in order."""
    tps, per_vm = [], None
    for line in lines:
        if found := TPS.match(line):
            tps.append(float(found.group(1)))
        elif found := RUNS.match(line.strip()):
            per_vm = int(found.group(1))
    if not tps:
        return []
    per_vm = per_vm or len(tps)
    return [tps[at:at + per_vm] for at in range(0, len(tps), per_vm)]


def logs(results, case):
    """The logs of `case` in `results`, a test-gce.sh results directory: each round's, in order."""
    root = Path(results) / "e2e" / case
    single = root / case / "log.txt"
    if single.exists():
        return [single]
    rounds = sorted(root.glob(f"round-*/{case}/log.txt"),
                    key=lambda path: int(path.parent.parent.name.split("-")[1]))
    return rounds


def measure(results):
    """Each case's rounds in `results`, by its JSON name: each round's VMs' runs' tps."""
    measured = {}
    for case, name in CASES.items():
        rounds = []
        for log in logs(results, case):
            with open(log, errors="replace") as text:
                rounds.append(runs(text))
        if any(rounds):
            measured[name] = [vms for vms in rounds if vms]
    return measured


def gates(measured, thresholds):
    """The gates of `measured`, as `measure` has it, against `thresholds`, by gate: each a dict of
    its name, the case, the value and its threshold, and whether it passed. Of each case's first
    round."""
    found = []

    def gate(name, case, value):
        found.append({"gate": name, "case": case, "value": round(value, 4),
                      "threshold": thresholds[name], "passed": value >= thresholds[name]})

    first = {case: rounds[0] for case, rounds in measured.items()}
    for case, vms in first.items():
        gate("decay", case, vms[0][-1] / vms[0][0])
        if len(vms) > 1:
            gate("density", case, statistics.median(vms[-1]) / statistics.median(vms[0]))
    if "dax" in first and "no-dax" in first:
        gate("dax", "dax", statistics.median(first["dax"][0])
             / statistics.median(first["no-dax"][0]))
    return found


def thresholds():
    """The gates' thresholds: the environment's FXDAX_GATE_<GATE>, or their defaults."""
    return {name: float(os.environ.get(f"FXDAX_GATE_{name.upper()}", default))
            for name, default in DEFAULTS.items()}


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--host", choices=["nested", "bare-metal"], default="nested")
    parser.add_argument("--json")
    parser.add_argument("results")
    args = parser.parse_args()
    measured = measure(args.results)
    if not measured:
        raise SystemExit(f"no pgbench runs in {args.results}")
    report = {"host": args.host, "tps": measured, "gates": gates(measured, thresholds())}
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
