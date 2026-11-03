# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/guest-mutants-gce.py, the hand mutants of the guest's DAX code."""

import unittest

import script

guest_mutants = script.load("guest-mutants-gce")


class Mutants(unittest.TestCase):
    def test_each_edit_finds_its_code_once(self):
        """TASK-44.19: the sources as they are hold each mutant's code once, which it changes: a
        mutant whose code moved would test nothing."""
        self.assertEqual(guest_mutants.check(set()), [])

    def test_an_edit_not_found_is_broken(self):
        """--check says which mutant's edit isn't in the sources, as CI runs it."""
        edits = [("no such code in dax.c", "")]
        with self.assertRaisesRegex(guest_mutants.Unfound, "found 0 times: 'no such code"):
            guest_mutants.mutated(guest_mutants.ROOT, edits)

    def test_mutants_change_the_code(self):
        """Each mutant's edit changes what it finds."""
        changed = [
            old != new for _, _, edits, *_ in guest_mutants.MUTANTS for old, new, *_ in edits
        ]
        self.assertEqual(changed, [True] * len(changed))

    def test_fates(self):
        """A run that passed missed its mutant; one whose suites failed a round, a single one
        too, killed it; one that failed otherwise, as a build that doesn't compile, is broken."""
        fate = guest_mutants.fate
        self.assertEqual(fate(0, "fxfs-guest-checks-dax: 2 rounds passed"), "MISSED")
        self.assertEqual(fate(1, "fxfs-guest-checks-dax: failed in round 2 of 3: see x"), "killed")
        self.assertEqual(fate(1, "fxfs-guest-checks-dax: failed in round 1 of 1: see x"), "killed")
        self.assertEqual(fate(1, "make: *** [fs/fxfs/dax.o] Error 1"), "BROKEN")


if __name__ == "__main__":
    unittest.main()
