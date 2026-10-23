# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""`cargo mutants`, as scripts/mutants.sh runs it, for its tests (test_mutants.py).

$FAKE_CARGO_MUTANTS/passes.json holds each pass's outcome: "sweep" for a run without features,
"shuttle" for one with --features shuttle, each the mutants it caught, missed, timed out on and
found unviable, by kind, and the status it ends with. A pass writes them where cargo-mutants 27.1.0
writes its report, in mutants.out of the --output directory, after it moves the report there to
mutants.out.old, deleting the older one (OutputDir::new). Each call's arguments are appended to
$FAKE_CARGO_MUTANTS/calls, a line of JSON each.

Usage: python3 fake_cargo_mutants.py mutants ARG...
"""

import json
import os
import shutil
import sys
from pathlib import Path

STATE = Path(os.environ["FAKE_CARGO_MUTANTS"])
KINDS = ["caught", "missed", "timeout", "unviable"]


def main(argv):
    if argv[:1] != ["mutants"]:
        sys.exit(f"fake cargo: no such call: {' '.join(argv)}")
    with open(STATE / "calls", "a") as calls:
        print(json.dumps(argv[1:]), file=calls)
    shuttle = "--features" in argv and argv[argv.index("--features") + 1] == "shuttle"
    outcome = json.loads((STATE / "passes.json").read_text())["shuttle" if shuttle else "sweep"]
    parent = Path(argv[argv.index("--output") + 1])
    parent.mkdir(exist_ok=True)
    report, rotated = parent / "mutants.out", parent / "mutants.out.old"
    if report.exists():
        if rotated.exists():
            shutil.rmtree(rotated)
        report.rename(rotated)
    report.mkdir()
    for kind in KINDS:
        (report / f"{kind}.txt").write_text("".join(f"{name}\n" for name in outcome.get(kind, [])))
    sys.exit(outcome["status"])


if __name__ == "__main__":
    main(sys.argv[1:])
