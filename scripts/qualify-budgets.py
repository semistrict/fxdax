#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""The qualification's budgets (scripts/qualify.sh, TASK-330.4): what scripts/budgets.py found of
each part's extent daemons (the part's budgets.json), in the fs bench's shape, which
scripts/bench-ab.py compares, a pin's rounds against the next's (RESULTS/bench-budgets.json):

- of each part, the most anonymous memory one of its daemons held (`budgets.PART.max_anon`, MiB),
  what its daemons had their kernels write for each byte their guests wrote
  (`budgets.PART.io_per_guest_byte`), and the CPU time they spent for each MiB their guests wrote
  (`budgets.PART.cpu_per_guest_mib`, ms), of the daemons that say them;
- as failures, each budget a daemon passed (`budgets.BUDGET`); a part whose daemons spent more CPU
  for each guest MiB than CPU_BUDGETS gives it (`budgets.cpu`, TASK-382); each daemon that says
  nothing its budgets weigh, as a pin's from before them does (`budgets.unreported`, TASK-330.2),
  whose costs the cases' own measurements, its kernel's counts, still say; and each part whose run
  left no budgets.json (`budgets.not-run`): it didn't run, which scripts/bench-ab.py fails the A/B
  on.

    qualify-budgets.py RESULTS PART...    exits 1 if a budget failed, a daemon said none, or a part
                                          has no budgets.json
"""

import json
import sys
from pathlib import Path

# The most CPU time, in ms, a part's extent daemons may spend for each MiB their guests write
# (`process:`'s cpu-ns over `daemon:`'s written_raw_bytes, all its daemons' together), TASK-382:
# nothing held a daemon's CPU, and main's spent 1.55 times the pin's in the bucket soak, its
# uploads all sending their objects again and again to the full and failing bucket. As the
# qualification runs, alone on ubu1's measurement cores (fxjob --bench), TASK-330.4's A/B measured
# the sandboxes' daemons at 14.4-14.6 ms (16.7 in a round a neighbour's disk loader overlapped),
# and the soak's at 24.3-25.5 ms on main then, 16.2 ms on the pin. With writes taking turns while
# the bucket fails, the soak's daemon spent 11.2 ms even run as plain work on a loaded ubu1, its
# writes capped, where main's before spent 103 ms and the pin's 122 (TASK-382's notes).
CPU_BUDGETS = {"sandboxes": 20, "soak": 20}


def summary(results, parts):
    """The bench-shaped summary of `parts`' budgets.json under `results`."""
    measured, failures, failed = [], {}, []

    def fail(kind, said):
        failures[kind] = failures.get(kind, 0) + 1
        failed.append(said)

    for part in parts:
        path = results / part / "budgets.json"
        if not path.exists():
            fail("budgets.not-run", f"{part}: not run: no budgets.json, its run ended before its "
                                    "budgets")
            continue
        anon, io, raw, cpu_ns, cpu_raw = [], 0, 0, 0, 0
        for daemon in json.loads(path.read_text()):
            name = f"{part}/{daemon['daemon']}"
            if not daemon["checks"]:
                fail("budgets.unreported", f"{name}: says nothing its budgets weigh")
            for check in daemon["checks"]:
                if not check["passed"]:
                    fail(f"budgets.{check['budget']}", f"{name}: {check['budget']}: {check['says']}")
            figures = daemon["figures"]
            if "max_anon" in figures:
                anon.append(figures["max_anon"])
            if "written" in figures:
                io += figures["io_write_bytes"]
                raw += figures["written"]["raw"]
                if "cpu_ns" in figures:
                    cpu_ns += figures["cpu_ns"]
                    cpu_raw += figures["written"]["raw"]
        if anon:
            measured.append(result(f"budgets.{part}.max_anon", "MiB", max(anon) / (1 << 20)))
        if raw:
            measured.append(result(f"budgets.{part}.io_per_guest_byte", "x", io / raw))
        if cpu_raw:
            per_mib = cpu_ns / 1e6 / (cpu_raw / (1 << 20))
            measured.append(result(f"budgets.{part}.cpu_per_guest_mib", "ms", per_mib))
            budget = CPU_BUDGETS[part]
            if per_mib > budget:
                fail("budgets.cpu", f"{part}: cpu: its daemons spent {per_mib:.1f} ms of CPU for "
                                    f"each MiB their guests wrote, budget {budget} ms")
    return {"case": "budgets", "backend": "budgets", "results": measured, "failures": failures,
            "failed": failed}


def result(workload, unit, value):
    return {"workload": workload, "unit": unit, "higher_is_better": False,
            "values": [round(value, 3)]}


def main(argv):
    if len(argv) < 2:
        sys.exit("usage: qualify-budgets.py RESULTS PART...")
    results, parts = Path(argv[0]), argv[1:]
    unbudgeted = [part for part in parts if part not in CPU_BUDGETS]
    if unbudgeted:
        sys.exit(f"qualify-budgets.py: no CPU budget for {', '.join(unbudgeted)}: CPU_BUDGETS")
    report = summary(results, parts)
    (results / "bench-budgets.json").write_text(json.dumps(report, indent=1) + "\n")
    for said in report["failed"]:
        print(f"budgets: {said}", file=sys.stderr)
    return 1 if report["failed"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
