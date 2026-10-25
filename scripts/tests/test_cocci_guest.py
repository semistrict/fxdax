# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/cocci-guest.py, the semantic patches of the guest's bugs' shapes."""

import tempfile
import unittest
from pathlib import Path

import script

cocci = script.load("cocci-guest")
guest_mutants = script.load("guest-mutants-gce")

DAX = script.SCRIPTS.parent / guest_mutants.DAX


def mutant(name):
    """dax.c with the hand mutant `name`'s edits made."""
    text = DAX.read_text()
    for mutant_name, _, edits in guest_mutants.MUTANTS:
        if mutant_name == name:
            for old, new in edits:
                text = text.replace(old, new)
            return text
    raise KeyError(name)


class RegionLock(unittest.TestCase):
    RULE = cocci.RULES / "region-lock.cocci"

    def test_the_guests_dax_code_reads_regions_under_their_locks(self):
        self.assertEqual(cocci.matches(self.RULE, DAX), "")

    def test_task_99s_unlocked_read_is_flagged(self):
        """TASK-44.17: fxfs_dax_region_open looking at a region's `mapped` under the regions lock
        alone, as before TASK-99's fix (the mutant region_open_unlocked)."""
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "dax.c"
            path.write_text(mutant("region_open_unlocked"))
            found = cocci.matches(self.RULE, path)
        flagged = [line for line in found.splitlines() if line.startswith("-\t")]
        self.assertEqual(flagged, ["-\tif (!region->writable && !region->mapped) {"])
        self.assertIn("fxfs_dax_region_open", found)


if __name__ == "__main__":
    unittest.main()
