# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of the fs bench's A/B on a host (TASK-276): scripts/lib/bench-ab-linux-host.sh's order of
the sides' rounds, scripts/bench-ab-gce.sh's sides, and scripts/test-e2e.sh's modes of artifacts
built once and runs of them, which refuse what they can't serve before anything is built."""

import subprocess
import tempfile
import unittest
from pathlib import Path

from script import SCRIPTS


def bash(command, env=None):
    """Runs `command` in bash: its status, output and errors."""
    done = subprocess.run(["/bin/bash", "-c", command], capture_output=True, text=True,
                          env={"PATH": "/usr/bin:/bin", **(env or {})})
    return done.returncode, done.stdout, done.stderr


class Rounds(unittest.TestCase):
    def test_the_sides_take_turns_going_first(self):
        host = SCRIPTS / "lib" / "bench-ab-linux-host.sh"
        status, out, _ = bash(f"source {host}; ab_order 4")
        self.assertEqual((status, out), (0, "a b\nb a\na b\nb a\n"))

    def test_sourced_it_does_nothing_but_define(self):
        host = SCRIPTS / "lib" / "bench-ab-linux-host.sh"
        status, out, err = bash(f"source {host}; echo sourced")
        self.assertEqual((status, out, err), (0, "sourced\n", ""))


class Sides(unittest.TestCase):
    def side(self, spec):
        # The driver's function alone, without its sourcing of gce.sh, which asks gcloud.
        driver = (SCRIPTS / "bench-ab-gce.sh").read_text()
        start = driver.index("side() {")
        function = driver[start:driver.index("\n}\n", start) + 3]
        return bash(f"{function}\nside '{spec}'")

    def test_a_side_is_a_name_and_a_tree(self):
        self.assertEqual(self.side("base=/src/fxdax base"), (0, "base /src/fxdax base\n", ""))

    def test_a_side_without_a_name_is_refused(self):
        status, _, err = self.side("/src/fxdax")
        self.assertEqual(status, 2)
        self.assertIn("a side is NAME=TREE", err)


class Artifacts(unittest.TestCase):
    """test-e2e.sh's modes, refused before any build, on any host."""

    def e2e(self, **env):
        results = tempfile.mkdtemp()
        return bash(f"{SCRIPTS / 'test-e2e.sh'} fxfs-guest",
                    {"FXDAX_E2E_RESULTS": results, **env})

    def test_building_artifacts_and_running_prebuilt_ones_are_one_or_the_other(self):
        status, _, err = self.e2e(FXDAX_E2E_BUILD_ONLY="/a", FXDAX_E2E_PREBUILT="/b")
        self.assertEqual(
            (status, err), (2, "FXDAX_E2E_BUILD_ONLY and FXDAX_E2E_PREBUILT: one or the other\n"))

    def test_prebuilt_artifacts_take_no_build_per_run(self):
        for mode in ["FXDAX_E2E_PREBUILT", "FXDAX_E2E_BUILD_ONLY"]:
            status, _, err = self.e2e(**{mode: "/b"}, FXDAX_FIRECRACKER_SECCOMP="log")
            self.assertEqual(
                (status, err), (2, "prebuilt artifacts take no FXDAX_FIRECRACKER_SECCOMP=log\n"),
                mode)

    def test_prebuilt_artifacts_are_jailed_by_their_own_jailer(self):
        """TASK-284: artifacts built under a seccomp filter hold the jailer, which a jailed run of
        them takes; those built with none hold none, and a jailed run of them is refused."""
        unjailed = tempfile.mkdtemp()
        (Path(unjailed) / "artifacts.txt").write_text("seccomp none\n")
        status, _, err = self.e2e(FXDAX_E2E_PREBUILT=unjailed, FXDAX_FIRECRACKER_JAIL="1")
        self.assertEqual((status, err), (2, f"{unjailed} holds no jailer, built with no seccomp "
                                            "filter: FXDAX_FIRECRACKER_JAIL needs one\n"))
        text = (SCRIPTS / "test-e2e.sh").read_text()
        self.assertIn('export FXDAX_FIRECRACKER_JAILER="$prebuilt/jailer"', text)
        self.assertIn('if [[ -n $build_only || -n ${FXDAX_FIRECRACKER_JAIL:-} ]]; then\n'
                      '            env "$musl_cflags" "$firecracker_cargo" build --release -p jailer',
                      text)
        self.assertIn("artifacts+=(jailer)", text)

    def test_a_directory_of_no_artifacts_is_refused(self):
        empty = tempfile.mkdtemp()
        status, _, err = self.e2e(FXDAX_E2E_PREBUILT=empty)
        self.assertEqual(
            (status, err), (2, f"{empty} holds no artifacts (FXDAX_E2E_BUILD_ONLY makes them)\n"))


if __name__ == "__main__":
    unittest.main()
