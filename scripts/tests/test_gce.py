# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/lib/gce.sh's gce_host_command: the command a GCE driver runs a host script
with, and the environment it runs in. bash builds the command, as a driver does, and another bash,
as the host's shell, takes it apart into the words sudo runs."""

import subprocess
import unittest

from script import SCRIPTS

GCE = SCRIPTS / "lib" / "gce.sh"
HOST_HOME = "/host/home"
# What follows sudo env's variables: the host script and its arguments.
COMMAND = [
    "timeout",
    "--signal=TERM",
    "--kill-after=30s",
    "6h",
    "bash",
    "run-x/source/scripts/lib/test-linux-host.sh",
    "run-x/source",
    "run-x/results",
]


class HostCommand(unittest.TestCase):
    def host_command(self, environment, *args):
        """The words the host runs for `gce_host_command run-x ARGS...`, run in `environment`:
        sudo env's variables, sorted, and the command they run."""
        built = subprocess.run(
            [
                # macOS's 3.2 there, as a driver runs in.
                "/bin/bash",
                "-c",
                'set -euo pipefail; project=p limit=6h; source "$0"; gce_host_command run-x "$@"',
                GCE,
                *args,
            ],
            env={"PATH": "/usr/bin:/bin", "HOME": "/laptop/home", **environment},
            capture_output=True,
            text=True,
        )
        self.assertEqual((built.returncode, built.stderr), (0, ""))
        words = subprocess.run(
            ["/bin/bash", "-c", 'eval "set -- $0"; printf "%s\\0" "$@"', built.stdout],
            env={"PATH": "/usr/bin:/bin", "HOME": HOST_HOME},
            capture_output=True,
            text=True,
            check=True,
        ).stdout.split("\0")[:-1]
        self.assertEqual(words[:2], ["sudo", "env"])
        command = words.index("timeout")
        return sorted(words[2:command]), words[command:]

    def test_every_variable_reaches_the_host(self):
        self.assertEqual(
            self.host_command(
                {
                    "FXDAX_FOLLOW_SECONDS": "600",
                    "FXDAX_PJDFSTEST_TESTS": "chmod chown",
                    "FXDAX_BENCH_WATCHDOG_SECS": "900",
                    "FXDAX_ZSTD_LEVEL": "3",
                    "FXDAX_BENCH_PGBENCH_SCALE": "50",
                    "FXDAX_REMOTE_LATENCY_MS": "25",
                    "PJDFSTEST_COMMIT": "85a8aea9",
                    # A driver's own, which does nothing there.
                    "FXDAX_GCE_ZONE": "us-east4-a",
                    "OTHER": "stays",
                },
                "test-linux-host.sh",
                "fxfs-guest fxfs-guest-dax",
                "a",
            ),
            (
                [
                    "FXDAX_BENCH_PGBENCH_SCALE=50",
                    "FXDAX_BENCH_WATCHDOG_SECS=900",
                    "FXDAX_FOLLOW_SECONDS=600",
                    "FXDAX_GCE_ZONE=us-east4-a",
                    "FXDAX_PJDFSTEST_TESTS=chmod chown",
                    "FXDAX_REMOTE_LATENCY_MS=25",
                    "FXDAX_ZSTD_LEVEL=3",
                    "PJDFSTEST_COMMIT=85a8aea9",
                ],
                COMMAND + ["fxfs-guest fxfs-guest-dax", "a"],
            ),
        )

    def test_what_names_this_machine_stays_here(self):
        here = {
            name: "/laptop/" + name
            for name in [
                "FXDAX_DOCKER_STATIC",
                "FXDAX_BUSYBOX",
                "FXDAX_FIRECRACKER_SECCOMP_FILTER",
                "FXDAX_FIRECRACKER_JAILER",
                "FXDAX_EXTENTD_GRPC",
                "FXDAX_EXTENTD_STATS",
                "FXDAX_EXTENTD_METRICS",
                "FXDAX_EXTENTD_ROOTS",
                "FXDAX_EXTENTD_TENANTS",
                "FXDAX_BENCH_RESULTS",
            ]
        }
        self.assertEqual(
            self.host_command({**here, "FXDAX_EXTENTD_CACHE": "/cache"}, "test-linux-host.sh"),
            (["FXDAX_EXTENTD_CACHE=/cache"], COMMAND),
        )

    def test_empty_values_are_not_sent(self):
        empty = {"FXDAX_ZSTD_LEVEL": "", "FXDAX_BENCH_PERF": ""}
        self.assertEqual(self.host_command(empty, "test-linux-host.sh"), ([], COMMAND))

    def test_values_arrive_as_they_are(self):
        note = "a 'quoted' \"note\": $HOME, `id`, \\back\\slashes\\, *?, ×\nand a line"
        self.assertEqual(
            self.host_command({"FXDAX_BENCH_NOTE": note}, "test-linux-host.sh", note),
            ([f"FXDAX_BENCH_NOTE={note}"], COMMAND + [note]),
        )

    def test_the_guest_kernel_is_the_copy_on_the_host(self):
        self.assertEqual(
            self.host_command({"FXDAX_GUEST_KERNEL": "/laptop/vmlinux"}, "test-linux-host.sh"),
            ([f"FXDAX_GUEST_KERNEL={HOST_HOME}/run-x/guest-kernel"], COMMAND),
        )

    def test_a_path_in_home_is_in_the_hosts(self):
        self.assertEqual(
            self.host_command({"FXDAX_EXTENTD_CACHE": "~/cache"}, "test-linux-host.sh"),
            ([f"FXDAX_EXTENTD_CACHE={HOST_HOME}/cache"], COMMAND),
        )

    def test_the_callers_values_replace_this_machines(self):
        self.assertEqual(
            self.host_command(
                {"FXDAX_EXTENTD_REMOTE": "gs://bucket/prefix", "FXDAX_ZSTD_LEVEL": "3"},
                "FXDAX_EXTENTD_REMOTE=gs://bucket/prefix/run-x",
                "FXDAX_MIGRATE_BUNDLE=~/run-x/bundle",
                # Empty, unset there.
                "FXDAX_ZSTD_LEVEL=",
                "test-linux-host.sh",
                "FXDAX_NOT=an argument",
            ),
            (
                [
                    "FXDAX_EXTENTD_REMOTE=gs://bucket/prefix/run-x",
                    f"FXDAX_MIGRATE_BUNDLE={HOST_HOME}/run-x/bundle",
                ],
                COMMAND + ["FXDAX_NOT=an argument"],
            ),
        )

    def test_nothing_to_send_and_no_arguments(self):
        self.assertEqual(self.host_command({}, "test-linux-host.sh"), ([], COMMAND))


if __name__ == "__main__":
    unittest.main()
