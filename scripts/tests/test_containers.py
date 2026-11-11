# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/lib/containers.sh's healthy_container (TASK-239): a container is ready when
its engine says its health check passed, which it waits for, and not before; one that never says so
fails in time, its log said and the container stopped. And the scripts that start servers so wait,
probing nothing from outside: a probe of RustFS retrying all errors into /dev/null exited 23 on
Linux (CI run 42423429301), as curl couldn't truncate what it had written."""

import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

from script import SCRIPTS

FAKE = Path(__file__).resolve().parent / "fake_container_engine.py"


class HealthyContainer(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.state = Path(self.dir.name)
        engine = self.state / "docker"
        engine.write_text(f"#!/bin/sh\nexec {sys.executable} {FAKE} \"$@\"\n")
        engine.chmod(0o755)
        self.engine = engine

    def tearDown(self):
        self.dir.cleanup()

    def start(self, events, within=5):
        """healthy_container's status, output and error, the engine's health events `events`."""
        (self.state / "events").write_text("".join(f"{event}\n" for event in events))
        script = (
            f'source "{SCRIPTS}/lib/containers.sh"; '
            f'healthy_container "{self.engine}" "curl -fsS http://127.0.0.1:9000/health/ready" '
            f"-p 127.0.0.1:1234:9000 the-image serve"
        )
        env = dict(os.environ, FAKE_ENGINE=str(self.state), HEALTHY_WITHIN=str(within))
        began = time.monotonic()
        done = subprocess.run(["bash", "-c", script], capture_output=True, text=True, env=env,
                              timeout=60)
        self.took = time.monotonic() - began
        return done.returncode, done.stdout, done.stderr

    def calls(self):
        return (self.state / "calls").read_text().splitlines()

    def test_a_container_is_ready_once_its_engine_says_it_is_healthy(self):
        status = self.start(["health_status: starting", "health_status: unhealthy",
                             "health_status: healthy"])
        self.assertEqual(status, (0, "container-1\n", ""))
        run, events = self.calls()
        self.assertEqual(run, "run -d --rm --health-cmd curl -fsS http://127.0.0.1:9000/health/ready "
                              "--health-interval 1s --health-retries 120 -p 127.0.0.1:1234:9000 "
                              "the-image serve")
        self.assertRegex(events, r"^events --since \d+ --filter container=container-1 "
                                 r"--filter event=health_status --format \{\{\.Action\}\}$")
        self.assertLess(self.took, 5, "it waited past the healthy event")

    def test_a_container_never_healthy_fails_in_time_stopped(self):
        status = self.start(["health_status: starting", "health_status: unhealthy"], within=1)
        self.assertEqual(status, (1, "", "container container-1 didn't say it was healthy within "
                                         "1 s:\nthe server's log\n"))
        self.assertEqual(self.calls()[-2:], ["logs container-1", "stop container-1"])
        self.assertLess(self.took, 5)

    def test_the_servers_scripts_wait_for_their_health_and_probe_nothing(self):
        text = (SCRIPTS / "test-azurite.sh").read_text()
        self.assertIn("healthy_container \"$engine\" 'nc -z 127.0.0.1 10000'", text)
        self.assertNotIn("--retry", text)
        # RustFS is a process of its release's binary, not a container, whose readiness it asks
        # directly, each probe once (TASK-330.5).
        text = (SCRIPTS / "lib" / "rustfs.sh").read_text()
        self.assertNotIn("container_engine", text)
        self.assertIn('curl -fsS -o /dev/null --max-time 2 "$AWS_ENDPOINT/health/ready"', text)
        # The scripts that keep what they test in RustFS's bucket start it so, through the lib.
        for name in ["test-rustfs.sh", "test-e2e-rustfs.sh"]:
            text = (SCRIPTS / name).read_text()
            self.assertIn('source "$root/scripts/lib/rustfs.sh"', text, name)
            self.assertNotIn("healthy_container", text, name)


if __name__ == "__main__":
    unittest.main()
