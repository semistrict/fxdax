# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/lib/patches.sh's patch_well_formed: the mutant scripts' check that a patch
applies with no fuzz as GNU patch takes it, which BSD patch (macOS) does not check (TASK-395)."""

import subprocess
import tempfile
import unittest
from pathlib import Path

from script import SCRIPTS

LINES = [f"line {i}" for i in range(1, 21)]


def hunk(header, body):
    """A patch of one hunk to f.txt."""
    return "# Kind: test\n--- a/f.txt\n+++ b/f.txt\n" + header + "\n" + "".join(f"{line}\n" for line in body)


class WellFormed(unittest.TestCase):
    def setUp(self):
        self.directory = Path(tempfile.mkdtemp())
        (self.directory / "f.txt").write_text("".join(f"{line}\n" for line in LINES))

    def check(self, patch):
        """patch_well_formed over `patch`, from the directory f.txt is in: its status and errors."""
        (self.directory / "m.patch").write_text(patch)
        done = subprocess.run(
            ["/bin/bash", "-c", f'source "{SCRIPTS}/lib/patches.sh"; patch_well_formed m.patch'],
            cwd=self.directory,
            capture_output=True,
            text=True,
        )
        return done.returncode, done.stderr

    def test_a_hunk_with_even_context_is_well_formed(self):
        body = [" line 8", " line 9", " line 10", "-line 11", "+eleven", " line 12", " line 13", " line 14"]
        self.assertEqual(self.check(hunk("@@ -8,7 +8,7 @@", body)), (0, ""))

    def test_a_hunk_with_more_lines_than_its_header_counts_is_refused(self):
        # The header counts 7 of the 8 lines it has, as a patch re-anchored by hand did: BSD patch
        # applied it, GNU patch didn't.
        body = [" line 7", " line 8", " line 9", " line 10", "-line 11", "+eleven", " line 12", " line 13", " line 14"]
        self.assertEqual(
            self.check(hunk("@@ -7,7 +7,7 @@", body)),
            (
                1,
                "m.patch: the hunk at line 4 has 4 lines of context before its changes and 2 after, "
                "away from the start and end of f.txt\n"
                "m.patch: line 13 is past its hunk, as its header counts it\n",
            ),
        )

    def test_a_hunk_with_fewer_lines_than_its_header_counts_is_refused(self):
        body = [" line 9", " line 10", "-line 11", "+eleven", " line 12", " line 13"]
        self.assertEqual(
            self.check(hunk("@@ -9,6 +9,6 @@", body)),
            (1, "m.patch: the hunk at line 4 has fewer lines than its header counts\n"),
        )

    def test_uneven_context_away_from_the_files_ends_is_refused(self):
        body = [" line 7", " line 8", " line 9", " line 10", "-line 11", "+eleven", " line 12", " line 13", " line 14"]
        self.assertEqual(
            self.check(hunk("@@ -7,7 +7,7 @@", body[:-1])),
            (
                1,
                "m.patch: the hunk at line 4 has 4 lines of context before its changes and 2 after, "
                "away from the start and end of f.txt\n",
            ),
        )

    def test_uneven_context_at_the_files_start_or_end_is_well_formed(self):
        start = ["-line 1", "+one", " line 2", " line 3", " line 4"]
        self.assertEqual(self.check(hunk("@@ -1,4 +1,4 @@", start)), (0, ""))
        end = [" line 17", " line 18", " line 19", "-line 20", "+twenty"]
        self.assertEqual(self.check(hunk("@@ -17,4 +17,4 @@", end)), (0, ""))


if __name__ == "__main__":
    unittest.main()
