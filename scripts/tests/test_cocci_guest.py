# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/cocci-guest.py, the semantic patches of the guest's bugs' shapes."""

import tempfile
import unittest
from pathlib import Path

import script

cocci = script.load("cocci-guest")
DAX = script.SCRIPTS.parent / "guest" / "linux" / "fs" / "fxfs" / "dax.c"


# fxfs_dax_region_open with TASK-99's race: as it was before 7caa43578, its region's lock taken out.
TASK_99_REGION_OPEN = """\
static void fxfs_dax_region_open(struct fxfs_dax *dax, struct fxfs_dax_region *region)
{
\tfxfs_device_assert_in_gate(dax->dev);
\tfxfs_dax_delay();
\tspin_lock(&dax->regions_lock);
\tif (!region->writable && !region->mapped) {
\t\tregion->writable = true;
\t\tlist_add_tail(&region->writable_list, &dax->writable_regions);
\t}
\tspin_unlock(&dax->regions_lock);
}
"""


class RegionLock(unittest.TestCase):
    RULE = cocci.RULES / "region-lock.cocci"

    def test_the_guests_dax_code_reads_regions_under_their_locks(self):
        self.assertEqual(cocci.matches(self.RULE, DAX), "")

    def test_task_99s_unlocked_read_is_flagged(self):
        """TASK-44.17: fxfs_dax_region_open looking at a region's `mapped` under the regions lock
        alone, as before TASK-99's fix. Its code as it was: since 7caa43578 it opens only regions
        nothing has mapped, and takes no region's lock for a mutant to take out."""
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "dax.c"
            path.write_text(TASK_99_REGION_OPEN)
            found = cocci.matches(self.RULE, path)
        flagged = [line for line in found.splitlines() if line.startswith("-\t")]
        self.assertEqual(flagged, ["-\tif (!region->writable && !region->mapped) {"])
        self.assertIn("fxfs_dax_region_open", found)


if __name__ == "__main__":
    unittest.main()
