#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""The qualification's budgets (scripts/qualify.sh, TASK-330.4): what scripts/budgets.py found of
each part's extent daemons (the part's budgets.json), in the fs bench's shape, which
scripts/bench-ab.py compares, a pin's rounds against the next's (RESULTS/bench-budgets.json):

- of each part, the most anonymous memory one of its daemons held (`budgets.PART.max_anon`, MiB),
  and what its daemons had their kernels write for each byte their guests wrote
  (`budgets.PART.io_per_guest_byte`), of the daemons that say them;
- as failures, each budget a daemon passed (`budgets.BUDGET`), each daemon that says nothing its
  budgets weigh, as a pin's from before them does (`budgets.unreported`, TASK-330.2), whose costs
  the cases' own measurements, its kernel's counts, still say; and each part whose run left no
  budgets.json (`budgets.not-run`): it didn't run, which scripts/bench-ab.py fails the A/B on.

    qualify-budgets.py RESULTS PART...    exits 1 if a budget failed, a daemon said none, or a part
                                          has no budgets.json
"""

import json
import sys
from pathlib import Path


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
        anon, io, raw = [], 0, 0
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
        if anon:
            measured.append(result(f"budgets.{part}.max_anon", "MiB", max(anon) / (1 << 20)))
        if raw:
            measured.append(result(f"budgets.{part}.io_per_guest_byte", "x", io / raw))
    return {"case": "budgets", "backend": "budgets", "results": measured, "failures": failures,
            "failed": failed}


def result(workload, unit, value):
    return {"workload": workload, "unit": unit, "higher_is_better": False,
            "values": [round(value, 3)]}


def main(argv):
    if len(argv) < 2:
        sys.exit("usage: qualify-budgets.py RESULTS PART...")
    results = Path(argv[0])
    report = summary(results, argv[1:])
    (results / "bench-budgets.json").write_text(json.dumps(report, indent=1) + "\n")
    for said in report["failed"]:
        print(f"budgets: {said}", file=sys.stderr)
    return 1 if report["failed"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
