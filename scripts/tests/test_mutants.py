# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/mutants.sh, the mutation testing of fxdax's own crates, its passes run by
fake_cargo_mutants.py: a sweep, then, for a crate with Shuttle tests, the sweep's missed again under
Shuttle."""

import json
import shlex
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from script import SCRIPTS

FAKE_CARGO_MUTANTS = Path(__file__).resolve().with_name("fake_cargo_mutants.py")
KINDS = ["caught", "missed", "timeout", "unviable"]
SWEEP = {
    "caught": ["c1", "c2", "c3"],
    "missed": ["m1", "m2"],
    "timeout": ["t1"],
    "unviable": ["u1"],
    "status": 2,
}
SHUTTLE = {"caught": ["m1"], "missed": ["m2"], "status": 2}


class Mutants(unittest.TestCase):
    def setUp(self):
        self.directory = Path(tempfile.mkdtemp())
        self.out = self.directory / "mutants"
        bin_ = self.directory / "bin"
        bin_.mkdir()
        for tool in ["cargo", "cargo-mutants"]:
            (bin_ / tool).write_text(
                f"#!/bin/sh\nexec {shlex.quote(sys.executable)} "
                f'{shlex.quote(str(FAKE_CARGO_MUTANTS))} "$@"\n'
            )
            (bin_ / tool).chmod(0o755)

    def mutants(self, passes, *crates):
        """Runs scripts/mutants.sh over `crates`, its passes as `passes` has them
        (fake_cargo_mutants.py): its status and output."""
        (self.directory / "passes.json").write_text(json.dumps(passes))
        (self.directory / "calls").write_text("")
        done = subprocess.run(
            ["/bin/bash", SCRIPTS / "mutants.sh", *crates],
            env={
                "PATH": f"{self.directory / 'bin'}:/usr/bin:/bin",
                "HOME": str(self.directory),
                "FXDAX_MUTANTS": str(self.out),
                "FAKE_CARGO_MUTANTS": str(self.directory),
            },
            capture_output=True,
            text=True,
        )
        return done.returncode, done.stdout

    def calls(self):
        return [json.loads(line) for line in (self.directory / "calls").read_text().splitlines()]

    def report(self, directory):
        """The report in `directory`, its lists by kind, as `passes` has them."""
        lists = {kind: (directory / f"{kind}.txt").read_text().split() for kind in KINDS}
        return {kind: names for kind, names in lists.items() if names}

    def test_each_pass_keeps_its_report(self):
        self.assertEqual(
            self.mutants({"sweep": SWEEP, "shuttle": SHUTTLE}, "extent_store"),
            (
                0,
                "extent_store: caught 3, missed 2, timeout 1, unviable 1\n"
                "extent_store, the missed under Shuttle: caught 1, missed 1, timeout 0, unviable 0\n",
            ),
        )
        without = {kind: names for kind, names in SWEEP.items() if kind != "status"}
        self.assertEqual(self.report(self.out / "extent_store" / "mutants.out"), without)
        self.assertEqual(
            self.report(self.out / "extent_store" / "shuttle" / "mutants.out"),
            {"caught": ["m1"], "missed": ["m2"]},
        )
        self.assertEqual(
            self.calls()[1],
            [
                "-p", "extent_store", "--test-tool", "nextest", "--jobs", "3",
                "--output", str(self.out / "extent_store" / "shuttle"),
                "--features", "shuttle", "-F", "^m1$|^m2$",
                "--", "-E", "test(/shuttle/) | binary(shuttle)",
            ],
        )

    def test_a_rerun_keeps_each_passes_report_before_it(self):
        self.mutants({"sweep": SWEEP, "shuttle": SHUTTLE}, "extent_store")
        again = {"caught": ["c1", "c2", "c3", "m1"], "missed": ["m2"], "status": 2}
        self.assertEqual(
            self.mutants({"sweep": again, "shuttle": {"missed": ["m2"], "status": 2}}, "extent_store"),
            (
                0,
                "extent_store: caught 4, missed 1, timeout 0, unviable 0\n"
                "extent_store, the missed under Shuttle: caught 0, missed 1, timeout 0, unviable 0\n",
            ),
        )
        crate = self.out / "extent_store"
        self.assertEqual(self.report(crate / "mutants.out")["caught"], ["c1", "c2", "c3", "m1"])
        self.assertEqual(self.report(crate / "mutants.out.old")["caught"], ["c1", "c2", "c3"])
        self.assertEqual(self.report(crate / "shuttle" / "mutants.out"), {"missed": ["m2"]})
        self.assertEqual(
            self.report(crate / "shuttle" / "mutants.out.old"), {"caught": ["m1"], "missed": ["m2"]}
        )

    def test_an_earlier_runs_shuttle_report_is_not_this_ones(self):
        self.mutants({"sweep": SWEEP, "shuttle": SHUTTLE}, "extent_store")
        every = {"caught": ["c1", "c2", "c3", "m1", "m2"], "status": 0}
        self.assertEqual(
            self.mutants({"sweep": every}, "extent_store"),
            (0, "extent_store: caught 5, missed 0, timeout 0, unviable 0\n"),
        )
        self.assertEqual(len(self.calls()), 1)

    def test_a_crate_without_shuttle_tests_has_its_sweep_alone(self):
        self.assertEqual(
            self.mutants({"sweep": SWEEP}, "extent_frame"),
            (0, "extent_frame: caught 3, missed 2, timeout 1, unviable 1\n"),
        )
        self.assertEqual(len(self.calls()), 1)

    def test_a_pass_that_failed_fails_the_run(self):
        self.assertEqual(
            self.mutants({"sweep": SWEEP, "shuttle": {"status": 101}}, "extent_store"),
            (
                1,
                "extent_store: caught 3, missed 2, timeout 1, unviable 1\n"
                "extent_store, the missed under Shuttle: caught 0, missed 0, timeout 0, unviable 0\n",
            ),
        )


if __name__ == "__main__":
    unittest.main()
