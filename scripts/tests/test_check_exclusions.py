# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/check-exclusions.py (TASK-149.20): each exclusion the tests make says why, and a
why naming a task names one still open."""

import tempfile
import unittest
from pathlib import Path

from script import SCRIPTS, load

check = load("check-exclusions")

STATES = {"TASK-143": "To Do", "TASK-150.21": "Done", "TASK-7": "Archived"}


class CheckExclusionsTest(unittest.TestCase):
    def test_ignored_tests_say_why(self):
        text = "#[test]\n#[ignore]\nfn a() {}\n#[ignore = \"a benchmark\"]\nfn b() {}\n"
        text += "#[cfg_attr(feature = \"x\", ignore)]\nfn c() {}\n"
        self.assertEqual(
            check.bare_ignores(Path("crates/x/src/lib.rs"), text),
            [
                'crates/x/src/lib.rs:2: an ignored test says no why: #[ignore = "why"]',
                'crates/x/src/lib.rs:6: an ignored test says no why: #[ignore = "why"]',
            ],
        )

    def test_fuchsias_files_keep_upstreams_ways(self):
        text = "// Copyright 2022 The Fuchsia Authors. All rights reserved.\n#[ignore]\nfn a() {}\n"
        self.assertEqual(check.bare_ignores(Path("crates/fxfs/src/a.rs"), text), [])

    def test_known_failures_name_an_open_task(self):
        text = (
            "# A comment.\n\nTASK-143 chmod/00.t\nTASK-150.21 WARN at fs/dax.c:416\n"
            "TASK-7 open/07.t\nTASK-99 rename/09.t\nchown/00.t\n"
        )
        self.assertEqual(
            check.known_list("known", text, STATES),
            [
                "known:4: its task isn't open (TASK-150.21, done): drop the line",
                "known:5: its task isn't open (TASK-7, archived): drop the line",
                "known:6: its task isn't open (TASK-99, no such task): drop the line",
                "known:7: names no task first: chown/00.t",
            ],
        )

    def test_xfstests_known_failures_say_why(self):
        text = """\
    const KNOWN_FAILURES: [&str; 4] = [
        // An extent store reserves nothing.
        "generic/009",
        "generic/092",

        "generic/301",
        // FIEMAP doesn't say which extents a reflink shares (TASK-143).
        "generic/372",
        // Fixed by TASK-150.21.
        "generic/401",
    ];
"""
        self.assertEqual(
            check.xfstests_known("x.rs", text, STATES),
            [
                'x.rs:6: "generic/301" is under no comment saying why',
                "x.rs:9: a known failure's task isn't open (TASK-150.21, done)",
            ],
        )

    def test_skipped_pjdfstests_name_open_tasks(self):
        text = 'const FXFS_SKIPPED: &[(&str, &str)] = &[("chmod/00.t", "TASK-150.21: fixed")];'
        self.assertEqual(
            check.pjdfstest_skipped("p.rs", text, STATES),
            ["p.rs: a skipped test's task isn't open (TASK-150.21, done)"],
        )

    def test_unkillable_mutants_say_why(self):
        silent = "# Kernel: sim\n# Killer: none\n#\n# A bug.\n\n--- a/fs/dax.c\n+++ b/fs/dax.c\n"
        equivalent = silent.replace("# A bug.", "# A bug. Equivalent since TASK-147.")
        killed = silent.replace("Killer: none", "Killer: scenarios moves")
        self.assertEqual(
            check.unkillable("m.patch", silent),
            ["m.patch: Killer: none, and its header says no why (equivalent, or out of every oracle's reach)"],
        )
        self.assertEqual(check.unkillable("m.patch", equivalent), [])
        self.assertEqual(check.unkillable("m.patch", killed), [])

    def test_task_states_are_the_backlogs(self):
        with tempfile.TemporaryDirectory() as root:
            tasks = Path(root, "backlog", "tasks")
            tasks.mkdir(parents=True)
            (tasks / "task-3 - a.md").write_text("---\nid: TASK-3\nstatus: Done\n---\n\nBody: x\n")
            (tasks / "task-4 - b.md").write_text("---\nid: task-4\nstatus: 'In Progress'\n---\n")
            archive = Path(root, "backlog", "archive", "tasks")
            archive.mkdir(parents=True)
            (archive / "task-5 - c.md").write_text("---\nid: TASK-5\nstatus: To Do\n---\n")
            self.assertEqual(
                check.task_states(Path(root)),
                {"TASK-3": "Done", "TASK-4": "In Progress", "TASK-5": "Archived"},
            )

    def test_the_tree_says_why_of_everything_it_leaves_out(self):
        self.assertEqual(check.check(SCRIPTS.parent), [])

    def test_the_scripts_part_runs_it(self):
        self.assertIn('"$root/scripts/check-exclusions.py"', (SCRIPTS / "test.sh").read_text())


if __name__ == "__main__":
    unittest.main()
