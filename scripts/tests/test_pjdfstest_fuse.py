# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/pjdfstest-fuse.py (TASK-149.38): the failures it reads from prove's summary,
its list of known ones, what it fails on, the pin it builds, and CI's job running it as root."""

import re
import unittest

from script import SCRIPTS, load

fuse = load("pjdfstest-fuse")

# prove's summary of a run with failures, as prove -r writes it.
PROVE = """\
/w/pjdfstest/tests/chmod/00.t ........ ok
/w/pjdfstest/tests/chown/00.t ........ Failed 3/1323 subtests

Test Summary Report
-------------------
/w/pjdfstest/tests/chown/00.t      (Wstat: 0 Tests: 1323 Failed: 3)
  Failed tests:  7, 400-401
/w/pjdfstest/tests/open/07.t       (Wstat: 256 (exited 1) Tests: 25 Failed: 0)
  Non-zero exit status: 1
  Parse errors: Bad plan.  You planned 25 tests but ran 3.
/w/pjdfstest/tests/rename/09.t     (Wstat: 0 Tests: 2353 Failed: 12)
  Failed tests:  1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11
                12
  TODO passed:   55
Files=237, Tests=8789, 120 wallclock secs
Result: FAIL
"""


class PjdfstestFuseTest(unittest.TestCase):
    def test_failures_are_read_from_proves_summary(self):
        expected = {"chown/00.t:7", "chown/00.t:400", "chown/00.t:401", "open/07.t"}
        expected |= {f"rename/09.t:{n}" for n in range(1, 13)}
        self.assertEqual(fuse.failures(PROVE), expected)
        self.assertEqual(fuse.failures("All tests successful.\nResult: PASS\n"), set())

    def test_the_known_list_says_each_failures_task(self):
        listed = fuse.known("# a comment\n\nTASK-1.2 chown/00.t:7 why\nTASK-3 open/07.t\n")
        self.assertEqual(listed, {"chown/00.t:7": "TASK-1.2", "open/07.t": "TASK-3"})
        with self.assertRaises(ValueError):
            fuse.known("chown/00.t:7 TASK-1\n")

    def test_it_fails_on_failures_unlisted_and_listed_ones_gone(self):
        found = {"chown/00.t:7", "chown/00.t:9", "open/07.t", "open/07.t:3"}
        listed = {"chown/00.t:7": "TASK-1", "open/07.t": "TASK-2", "mkdir/00.t:1": "TASK-3",
                  "rename/00.t": "TASK-4"}
        self.assertEqual(fuse.unexpected(found, listed, []),
                         (["chown/00.t:9"], ["mkdir/00.t:1", "rename/00.t"]))
        # Of the directories run alone.
        self.assertEqual(fuse.unexpected(found, listed, ["chown", "open"]), (["chown/00.t:9"], []))

    def test_the_pin_is_the_guest_cases(self):
        self.assertEqual(fuse.pin(), ("https://github.com/pjd/pjdfstest",
                                      "85a8aea9e685999ef0540392fd80535f873d7ff7"))
        # Its one owner, which every run of pjdfstest sources.
        for script in ["test-guest-kernel.sh", "lib/test-linux-host.sh"]:
            text = (SCRIPTS / script).read_text()
            self.assertIn("pjdfstest.sh\"", text, script)
            self.assertNotIn("85a8aea9", text, script)

    def test_every_known_failure_has_a_task(self):
        fuse.known(fuse.KNOWN.read_text())

    def test_ci_runs_it_as_root_on_fxdax_mount(self):
        ci = (SCRIPTS.parent / ".github" / "workflows" / "ci.yml").read_text()
        job = re.search(r"^  pjdfstest-fuse:\n(.*?)(?=^  \S|\Z)", ci, re.MULTILINE | re.DOTALL)
        self.assertIsNotNone(job, "no pjdfstest-fuse job")
        job = job.group(1)
        self.assertIn("cargo build --locked -p fxdax --bin fxdax-mount", job)
        self.assertRegex(job, r"sudo .*scripts/pjdfstest-fuse\.py target/debug/fxdax-mount")
        for package in ["fuse3", "autoconf", "automake"]:
            self.assertIn(package, job)


if __name__ == "__main__":
    unittest.main()
