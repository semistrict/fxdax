# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/guest_results.py, the guest tests' results as data (TASK-160.31): a run's
summary as its rounds end, what it says of them, and what a bucket keeps of a run whose host is
lost, here a bucket of fake_gcloud.py's."""

import json
import os
import socket
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import script
from test_gce import Project

guest_results = script.load("guest_results")

# The knobs a run is given, and what says where it writes, which aren't knobs.
GIVEN = {
    "FXDAX_E2E_ROUNDS": "3",
    "FXDAX_GUEST_CMDLINE": "fxfs.follow_apply_delay_us=200",
    "FXDAX_GUEST_KERNEL_BUILD": "release",
    "PJDFSTEST_COMMIT": "85a8aea",
    "FXDAX_GUEST_KERNEL": "/var/lib/fxdax-test/guest-kernel-release/out/x86_64/vmlinux",
    "FXDAX_E2E_RESULTS": "/home/x/run-1/results/e2e",
    "FXDAX_RESULTS_BUCKET": "gs://bucket/prefix/run-1/e2e",
    "HOME": "/home/x",
}
KNOBS = {
    "FXDAX_E2E_ROUNDS": "3",
    "FXDAX_GUEST_CMDLINE": "fxfs.follow_apply_delay_us=200",
    "FXDAX_GUEST_KERNEL_BUILD": "release",
    "PJDFSTEST_COMMIT": "85a8aea",
}


def outcome(case, round_, outcome_, failed_checks=(), reason=None):
    """A round's outcome, as the runner writes it (`runner test --outcome`)."""
    return {"case": case, "round": round_, "rounds": 3, "outcome": outcome_, "reason": reason,
            "failed_checks": list(failed_checks), "took_secs": 1.5, "limit_secs": 15,
            "commit": "abc123", "replay": f"FXDAX_E2E_TEST={case} scripts/test-gce.sh all",
            "knobs": KNOBS}


class GuestResults(unittest.TestCase):
    def setUp(self):
        self.results = Path(tempfile.mkdtemp()) / "e2e"
        # The run's environment alone, none of this one's FXDAX_* (FXDAX_TEST_PARTS, as CI's).
        self.environment = mock.patch.dict(
            os.environ, {**GIVEN, "FXDAX_BUILD_COMMIT": "abc123", "PATH": os.environ["PATH"]},
            clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        del os.environ["FXDAX_RESULTS_BUCKET"]

    def ended(self, case, *rounds):
        """`case`'s rounds, each ended as test-e2e.sh ends one."""
        (self.results / case).mkdir(exist_ok=True)
        for each in rounds:
            path = self.results / case / f"round-{each['round']}.json"
            path.write_text(json.dumps(each))
            guest_results.ended(self.results, case)

    def test_the_knobs_are_how_a_run_runs(self):
        self.assertEqual(guest_results.knobs({**GIVEN, "FXDAX_BUILD_COMMIT": "abc123"}),
                         {**KNOBS, "FXDAX_BUILD_COMMIT": "abc123"})
        # A kernel named, not built, is one.
        given = {name: value for name, value in GIVEN.items() if name != "FXDAX_GUEST_KERNEL_BUILD"}
        self.assertEqual(guest_results.knobs(given)["FXDAX_GUEST_KERNEL"], GIVEN["FXDAX_GUEST_KERNEL"])

    def test_a_summary_of_each_case_as_its_rounds_end(self):
        cases = ["fxfs-guest-dax", "fxfs-guest-checks-dax", "fxfs-guest-crash-dax", "extent-guest"]
        guest_results.start(self.results, 3, cases)
        self.assertEqual(json.loads((self.results / "run.json").read_text()),
                         {"commit": "abc123", "knobs": {**KNOBS, "FXDAX_BUILD_COMMIT": "abc123"},
                          "rounds": 3, "cases": cases})
        self.assertEqual([each["outcome"] for each in guest_results.load(self.results)["cases"]],
                         ["unrun"] * 4)
        self.ended("fxfs-guest-dax", *(outcome("fxfs-guest-dax", k, "pass") for k in [1, 2, 3]))
        self.ended("fxfs-guest-checks-dax", outcome("fxfs-guest-checks-dax", 1, "pass"),
                   outcome("fxfs-guest-checks-dax", 2, "fail", ["sparse", "acls"], "seed 7: …"))
        self.ended("fxfs-guest-crash-dax", outcome("fxfs-guest-crash-dax", 1, "pass"))
        summary = guest_results.load(self.results)
        self.assertEqual((summary["status"], summary["failed"]), (None, ["fxfs-guest-checks-dax"]))
        self.assertEqual(summary["cases"][1], {
            "case": "fxfs-guest-checks-dax", "outcome": "fail", "round": 2, "rounds_passed": 1,
            "failed_checks": ["sparse", "acls"], "reason": "seed 7: …",
            "replay": "FXDAX_E2E_TEST=fxfs-guest-checks-dax scripts/test-gce.sh all",
            "daemons": []})
        self.assertEqual(guest_results.report(summary), [
            "fxfs-guest-dax: passed 3 rounds",
            "fxfs-guest-checks-dax: failed in round 2 of 3, checks sparse, acls",
            "  replay: FXDAX_E2E_TEST=fxfs-guest-checks-dax scripts/test-gce.sh all",
            "fxfs-guest-crash-dax: stopped after round 1 of 3, which passed",
            "extent-guest: not run",
            "the run did not finish",
        ])
        guest_results.finish(self.results, 1)
        self.assertEqual(guest_results.load(self.results)["status"], 1)
        with mock.patch("sys.stdout"):
            self.assertEqual(guest_results.main(["report", str(self.results)]), 1)

    def test_each_daemon_says_what_it_downloaded_against_what_it_read(self):
        """TASK-165.12: each extent daemon's last counts (its stats file, FXDAX_EXTENTD_STATS, the
        run's and each case's own, `NAME-stats.txt`): the bytes it downloaded from its bucket
        against those its page-ins read of the bucket's data, through its part cache, and of its
        store's. A case's daemon with no part cache downloaded the same parts four times over (GCE
        r741: 2.3 GB)."""
        guest_results.start(self.results, 1, ["fxfs-guest-pgbench-dax-forked"])
        logs = self.results / "fxfs-guest-pgbench-dax-forked" / "fxfs-guest-pgbench-dax-forked"
        logs.mkdir(parents=True)
        (logs / "extentd-stats.txt").write_text(
            "object-store: get=541 get-range=16 head=0 down=1000\n"
            "daemon: maps=1 page-ins=9 page-in-bytes=2000 fetched-bytes=3000 reads=1\n"
            "object-store: get=541 get-range=16 head=0 down=774037\n"
            "followers: views=0\n"
            "daemon: maps=2 page-ins=123904 page-in-bytes=202076160 fetched-bytes=4728643584 "
            "reads=5523 read-bytes=226099200\n"
            "requests: open=2\n")
        (logs / "samples.txt").write_text("t=2 pending_upload_bytes=0\n")
        (self.results / "extentd-stats.txt").write_text(
            "object-store: get=3 down=2500000000\ndaemon: page-in-bytes=0 fetched-bytes=500000000\n")
        self.ended("fxfs-guest-pgbench-dax-forked",
                   outcome("fxfs-guest-pgbench-dax-forked", 1, "pass"))
        guest_results.finish(self.results, 0)
        summary = guest_results.load(self.results)
        self.assertEqual(summary["cases"][0]["daemons"], [
            {"daemon": "extentd", "downloaded": 774037, "bucket_read": 4728643584,
             "store_read": 202076160}])
        self.assertEqual(summary["daemon"], {"daemon": "extentd", "downloaded": 2500000000,
                                             "bucket_read": 500000000, "store_read": 0})
        self.assertEqual(guest_results.report(summary), [
            "fxfs-guest-pgbench-dax-forked: passed",
            "  extentd: downloaded 774.0 kB for 4.7 GB read of its bucket's, 202.1 MB of its "
            "store's",
            "the run's extentd: downloaded 2.5 GB for 500.0 MB read of its bucket's, 0 B of its "
            "store's",
            "the run ended with 0",
        ])

    def test_a_run_never_started_has_no_summary(self):
        self.assertIsNone(guest_results.load(self.results))
        with mock.patch("sys.stdout"):
            self.assertEqual(guest_results.main(["report", str(self.results)]), 1)

    def test_a_host_lost_mid_run_keeps_its_finished_cases(self):
        """AC #2: each case as it ended, in the bucket, which a run's driver fetches back once the
        host is gone, as the host had it but for what isn't results."""
        project = Project({})
        os.environ.update(project.environment, FXDAX_RESULTS_BUCKET="gs://bucket/p/run-1/e2e")
        guest_results.start(self.results, 1, ["fxfs-guest-dax", "fxfs-guest-checks-dax"])
        logs = self.results / "fxfs-guest-dax" / "fxfs-guest-dax"
        (logs / "rootfs" / "bin").mkdir(parents=True)
        (logs / "rootfs" / "bin" / "sh").write_text("not results")
        (logs / "log.txt").write_text("guest said\n")
        listening = socket.socket(socket.AF_UNIX)
        listening.bind(str(logs / "api.sock"))
        self.addCleanup(listening.close)
        self.ended("fxfs-guest-dax", outcome("fxfs-guest-dax", 1, "pass"))
        # The next case, running as the host goes: its files never reach the bucket.
        (self.results / "fxfs-guest-checks-dax").mkdir()
        (self.results / "fxfs-guest-checks-dax" / "partial.txt").write_text("lost\n")

        fetched = Path(tempfile.mkdtemp()) / "e2e"
        self.assertEqual(guest_results.main(["fetch", "gs://bucket/p/run-1/e2e", str(fetched)]), 0)
        self.assertEqual(sorted(str(path.relative_to(fetched)) for path in fetched.rglob("*")), [
            "fxfs-guest-dax",
            "fxfs-guest-dax/fxfs-guest-dax",
            "fxfs-guest-dax/fxfs-guest-dax/log.txt",
            "fxfs-guest-dax/round-1.json",
            "run.json",
            "summary.json",
        ])
        self.assertEqual((fetched / "fxfs-guest-dax/fxfs-guest-dax/log.txt").read_text(),
                         "guest said\n")
        summary = guest_results.load(fetched)
        self.assertEqual([each["outcome"] for each in summary["cases"]], ["pass", "unrun"])

    def test_a_failed_upload_leaves_the_run_going(self):
        project = Project({"failing": ["storage"]})
        os.environ.update(project.environment, FXDAX_RESULTS_BUCKET="gs://bucket/p")
        with mock.patch("sys.stderr") as stderr:
            guest_results.start(self.results, 1, ["fxfs-guest-dax"])
        said = "".join(call.args[0] for call in stderr.write.call_args_list)
        self.assertIn("guest_results: uploading run.json to gs://bucket/p: ERROR: "
                      "(gcloud.storage.cp) copying failed", said)
        self.assertEqual(guest_results.load(self.results)["cases"][0]["outcome"], "unrun")


if __name__ == "__main__":
    unittest.main()
