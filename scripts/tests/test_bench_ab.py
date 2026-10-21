# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/bench-ab.py: python3 -m unittest discover scripts/tests (scripts/test.sh)."""

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "bench-ab.py"
spec = importlib.util.spec_from_file_location("bench_ab", SCRIPT)
bench_ab = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench_ab)


def results(**workloads):
    """A results directory whose one case has `workloads`, each (higher_is_better, values)."""
    directory = Path(tempfile.mkdtemp())
    entries = [
        {"workload": name, "unit": "ops/s", "higher_is_better": higher, "values": values}
        for name, (higher, values) in workloads.items()
    ]
    (directory / "bench-fs-x.json").write_text(json.dumps({"results": entries}))
    return directory


class Table(unittest.TestCase):
    def test_pools_rounds_and_says_how_much_slower_b_is(self):
        a = [results(create=(True, [100, 110])), results(create=(True, [90]))]
        b = [results(create=(True, [50, 55, 45]))]
        self.assertEqual(
            bench_ab.table("a", a, "b", b).splitlines()[2],
            "| create | ops/s | 100 (90–110) | 50 (45–55) | 2.00× | no |",
        )

    def test_times_lower_are_better_and_ranges_overlap(self):
        a = [results(init=(False, [4.0, 3.0]))]
        b = [results(init=(False, [3.4, 3.8]))]
        self.assertEqual(
            bench_ab.table("a", a, "b", b).splitlines()[2],
            "| init | ops/s | 3.5 (3–4) | 3.6 (3.4–3.8) | 1.03× | yes |",
        )


if __name__ == "__main__":
    unittest.main()
