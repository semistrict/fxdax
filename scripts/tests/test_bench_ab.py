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


def cases(**backends):
    """A results directory with a case per backend, each {workload: (higher_is_better, values)}."""
    directory = Path(tempfile.mkdtemp())
    for backend, workloads in backends.items():
        entries = [
            {"workload": name, "unit": "IOPS", "higher_is_better": higher, "values": values}
            for name, (higher, values) in workloads.items()
        ]
        report = {"backend": backend, "results": entries}
        (directory / f"bench-fs-{backend}.json").write_text(json.dumps(report))
    return directory


class ControlTable(unittest.TestCase):
    """TASK-276: with a control, cases apart, and a difference real only past the control's drift."""

    def rows(self, a, b):
        return bench_ab.control_table("a", a, "b", b, "ext4").splitlines()[2:]

    def test_a_difference_past_the_controls_drift_is_real(self):
        a = [cases(ext4={"read": (True, [100, 104])}, fxfs={"read": (True, [50, 52])})]
        b = [cases(ext4={"read": (True, [98, 102])}, fxfs={"read": (True, [70, 74])})]
        self.assertEqual(
            self.rows(a, b),
            ["| fxfs | read | IOPS | 51 (50–52) | 72 (70–74) | 0.71× | 1.02× | real |"],
        )

    def test_a_difference_the_control_moves_as_much_is_within_drift(self):
        a = [cases(ext4={"read": (True, [100, 101])}, fxfs={"read": (True, [50, 51])})]
        b = [cases(ext4={"read": (True, [75, 76])}, fxfs={"read": (True, [40, 41])})]
        self.assertEqual(
            self.rows(a, b),
            ["| fxfs | read | IOPS | 50.5 (50–51) | 40.5 (40–41) | 1.25× | 1.33× | within drift |"],
        )

    def test_overlapping_ranges_are_within_drift_and_no_control_is_said(self):
        a = [cases(ext4={"read": (True, [100])}, fxfs={"read": (True, [50, 80]), "map": (False, [2])})]
        b = [cases(ext4={"read": (True, [100])}, fxfs={"read": (True, [60, 90]), "map": (False, [3])})]
        self.assertEqual(
            self.rows(a, b),
            [
                "| fxfs | map | IOPS | 2 (2–2) | 3 (3–3) | 1.50× | - | no control |",
                "| fxfs | read | IOPS | 65 (50–80) | 75 (60–90) | 0.87× | 1.00× | within drift |",
            ],
        )


if __name__ == "__main__":
    unittest.main()
