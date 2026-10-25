# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/guest-mutants-gce.py, the hand mutants of the guest's DAX code."""

import unittest

import script

guest_mutants = script.load("guest-mutants-gce")


class Mutants(unittest.TestCase):
    def test_each_edit_finds_its_code_once(self):
        """TASK-44.19: dax.c as it is holds each mutant's code once, which it changes: a mutant
        whose code moved would test nothing."""
        dax = (script.SCRIPTS.parent / guest_mutants.DAX).read_text()
        found = {
            name: [dax.count(old) for old, _ in edits] for name, _, edits in guest_mutants.MUTANTS
        }
        self.assertEqual(found, {name: [1] * len(edits) for name, _, edits in guest_mutants.MUTANTS})

    def test_mutants_change_the_code(self):
        """Each mutant's edit changes what it finds."""
        changed = [old != new for _, _, edits in guest_mutants.MUTANTS for old, new in edits]
        self.assertEqual(changed, [True] * len(changed))


if __name__ == "__main__":
    unittest.main()
