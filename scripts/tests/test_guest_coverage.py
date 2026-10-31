# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/guest-coverage.py, which reports what the guest checks ran of fs/fxfs."""

import unittest

import script

guest_coverage = script.load("guest-coverage")

GCOV = """\
        -:    0:Source:fs/fxfs/dax.c
function fxfs_dax_used called 12 returned 100% blocks executed 75%
       12:   10:static int fxfs_dax_used(int a)
        -:   11:{
       12:   12:	if (a)
branch  0 taken 12
branch  1 taken 0 (fallthrough)
    #####:   13:		return 1;
    #####:   14:		return 2;
       12:   15:	return 0;
function fxfs_dax_unused called 0 returned 0% blocks executed 0%
    #####:   20:static int fxfs_dax_unused(void)
branch  0 never executed
    #####:   21:	return 0;
"""


class Summarize(unittest.TestCase):
    def test_counts_and_what_never_ran(self):
        report = guest_coverage.summarize(GCOV)
        self.assertEqual(
            (report["run"], report["lines"], report["taken"], report["branches"]), (3, 7, 1, 3)
        )
        self.assertEqual(report["never_called"], ["fxfs_dax_unused"])
        self.assertEqual(report["unrun"], {"fxfs_dax_used": [13, 14]})

    def test_describes_runs_of_lines(self):
        self.assertEqual(
            guest_coverage.describe("dax", guest_coverage.summarize(GCOV)),
            [
                "dax.c: lines 3/7 (42.9%), branches taken 1/3 (33.3%)",
                "  never called: fxfs_dax_unused",
                "  fxfs_dax_used: not run: 13-14",
            ],
        )

    def test_spans_join_following_lines(self):
        self.assertEqual(guest_coverage.spans([1, 2, 3, 7, 9, 10]), "1-3,7,9-10")


if __name__ == "__main__":
    unittest.main()
