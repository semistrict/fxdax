# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of guest/mutants, the hand mutants of the guest's code, and of
scripts/guest-mutants-gce.py, which runs those that need a real kernel (TASK-160.32)."""

import tempfile
import unittest
from pathlib import Path

import script

guest_mutants = script.load("guest-mutants-gce")
KNOWN = guest_mutants.runs()


def mutant(header):
    """The mutant of a patch whose header is `header`."""
    with tempfile.TemporaryDirectory() as scratch:
        path = Path(scratch) / "a-mutant.patch"
        path.write_text(f"# Copyright\n#\n{header}\n# What it was.\n\n--- a/x\n+++ b/x\n")
        each = guest_mutants.mutant(path)
        return each, guest_mutants.unsound(each, KNOWN)


def summary(case, outcome, failed_checks=(), reason=None):
    """A run's summary of the one case `case`, as scripts/guest_results.py writes it."""
    return {"cases": [{"case": case, "outcome": outcome, "failed_checks": list(failed_checks),
                       "reason": reason}]}


class Mutants(unittest.TestCase):
    def test_each_mutant_is_sound_and_applies(self):
        """Each header names a kernel and a killer there is, and each real kernel's patch applies
        to the sources as they are: one whose code moved, or whose killer went, would test
        nothing (the simulator's apply in guest/sim/mutants.sh --check)."""
        self.assertEqual(guest_mutants.check(), [])

    def test_one_set_of_both_kinds(self):
        """The simulator's mutants, the model's and the real kernels' are one set, and each needs
        one."""
        kernels = {each.kernel for each in guest_mutants.mutants()}
        self.assertEqual(kernels, {"sim", "model", "release"})

    def test_what_the_runs_are(self):
        """What a killer may name, as the sources declare them."""
        self.assertIn("fxfs-guest-checks-dax-small", KNOWN["cases"])
        self.assertIn("window_held_killed", KNOWN["checks"])
        self.assertIn("interleave", KNOWN["sim"])
        self.assertIn("cow_swap_during_write", KNOWN["scenarios"])
        self.assertIn("the_models_past_drifts_from_fxfs", KNOWN["model"])

    def test_headers(self):
        """A header's kernel and killer, and what is wrong with one."""
        each, wrong = mutant("# Kernel: sim\n# Killer: scenarios cow_swap_during_write")
        self.assertEqual((each.name, each.kernel, each.killer, each.real, wrong),
                         ("a-mutant", "sim", ("scenarios", "cow_swap_during_write"), False, None))
        each, wrong = mutant("# Kernel: debug\n# Killer: fxfs-guest-checks-dax")
        self.assertEqual((each.killer, each.real, wrong), (("fxfs-guest-checks-dax",), True, None))
        each, wrong = mutant("# Kernel: model\n# Killer: fxfs-sim-diff the_models_past_drifts_from_fxfs")
        self.assertEqual((each.killer, each.real, wrong),
                         (("fxfs-sim-diff", "the_models_past_drifts_from_fxfs"), False, None))
        each, wrong = mutant("# Kernel: sim\n# Killer: none")
        self.assertEqual((each.killer, wrong), ((), None))
        cases = [
            ("# Killer: interleave",
             "its kernel is '', none of ['debug', 'kasan', 'model', 'release', 'sim']"),
            ("# Kernel: model\n# Killer: fxfs-sim-diff nope",
             "its killer 'fxfs-sim-diff nope' is no test of crates/fxfs-sim-diff"),
            ("# Kernel: model\n# Killer: scenarios cow_swap_during_write",
             "its killer 'scenarios cow_swap_during_write' is no test of crates/fxfs-sim-diff"),
            ("# Kernel: sim", "its header names no killer"),
            ("# Kernel: sim\n# Killer: nope", "its killer 'nope' is no test of guest/sim's"),
            ("# Kernel: sim\n# Killer: scenarios nope",
             "its killer 'scenarios nope' is no scenario of tests/scenarios"),
            ("# Kernel: release\n# Killer: fxfs-guest-nope", "its killer 'fxfs-guest-nope' is no guest case"),
            ("# Kernel: release\n# Killer: fxfs-guest-checks-dax nope",
             "its killer 'fxfs-guest-checks-dax nope' names no check of the guest's checks suites"),
        ]
        for header, said in cases:
            self.assertEqual(mutant(header)[1], said, header)

    def test_fates(self):
        """A mutant is killed only by its killer: its case failing or timing out, with the check
        named among those failed; MISSED if the case passed, or failed otherwise; BROKEN if the
        run failed with the case not failed, or never ran it (TASK-160.31)."""
        fate = guest_mutants.fate
        case, check = "fxfs-guest-checks-dax-small", "window_held_killed"
        killer = (case, check)
        self.assertEqual(fate(1, summary(case, "fail", [check, "sparse"]), killer),
                         ("killed", f"{case} fail: {check}, sparse"))
        self.assertEqual(fate(1, summary(case, "timeout", [check]), killer),
                         ("killed", f"{case} timeout: {check}"))
        self.assertEqual(fate(0, summary(case, "pass"), killer), ("MISSED", f"{case} passed"))
        self.assertEqual(fate(1, summary(case, "fail", ["sparse"]), killer),
                         ("MISSED", f"{case} failed, but not {check}: sparse"))
        self.assertEqual(fate(1, summary(case, "fail", [], "a kernel complained"), killer),
                         ("MISSED", f"{case} failed, but not {check}: a kernel complained"))
        self.assertEqual(fate(1, summary(case, "fail", [], "a kernel complained"), (case,)),
                         ("killed", f"{case} fail: a kernel complained"))
        self.assertEqual(fate(1, summary(case, "unrun"), killer),
                         ("BROKEN", f"the run ended with 1, {case} unrun"))
        self.assertEqual(fate(1, None, killer), ("BROKEN", f"the run ended with 1, {case} never ran"))


if __name__ == "__main__":
    unittest.main()
