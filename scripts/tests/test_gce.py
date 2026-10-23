# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/lib/gce.sh, which the GCE drivers source, and of the drivers.

gce_host_command builds the command a driver runs a host script with, and the environment it runs
in: bash builds the command, as a driver does, and another bash, as the host's shell, takes it
apart into the words sudo runs. The hosts' lives, and the jobs run on them, run on a project of
fake_gcloud.py's."""

import json
import re
import shlex
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from script import SCRIPTS

GCE = SCRIPTS / "lib" / "gce.sh"
FAKE_GCLOUD = Path(__file__).resolve().with_name("fake_gcloud.py")
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

    def test_this_machines_own_setup_stays_here(self):
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
        # Knobs for the host: where its stores keep their branches, and its daemons what they read
        # from them. And a path of this machine's build that the host scripts set themselves.
        host = {
            "FXDAX_EXTENTD_REMOTE": "gs://bucket/prefix",
            "FXDAX_S3_URL": "s3://bucket/test",
            "FXDAX_EXTENTD_CACHE": "/cache",
            "FXDAX_FIRECRACKER": "/laptop/firecracker",
        }
        self.assertEqual(
            self.host_command({**here, **host}, "test-linux-host.sh"),
            (
                [
                    "FXDAX_EXTENTD_CACHE=/cache",
                    "FXDAX_EXTENTD_REMOTE=gs://bucket/prefix",
                    "FXDAX_FIRECRACKER=/laptop/firecracker",
                    "FXDAX_S3_URL=s3://bucket/test",
                ],
                COMMAND,
            ),
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


class Project:
    """A project of fake_gcloud.py's, as `state` (fake_gcloud.py) has it, in a temporary
    directory, which is the home of the shells that run on it."""

    def __init__(self, state):
        self.directory = Path(tempfile.mkdtemp())
        (self.directory / "state.json").write_text(json.dumps(state))
        (self.directory / "calls").write_text("")
        (self.directory / "bin").mkdir()
        gcloud = self.directory / "bin" / "gcloud"
        gcloud.write_text(
            f'#!/bin/sh\nexec {shlex.quote(sys.executable)} {shlex.quote(str(FAKE_GCLOUD))} "$@"\n'
        )
        gcloud.chmod(0o755)
        self.environment = {
            "PATH": f"{self.directory / 'bin'}:/usr/bin:/bin",
            "HOME": str(self.directory),
            "FAKE_GCLOUD": str(self.directory),
        }

    def state(self):
        return json.loads((self.directory / "state.json").read_text())

    def calls(self):
        """The calls of gcloud, each its words."""
        return (self.directory / "calls").read_text().splitlines()

    def run(self, script):
        """Runs `script` in macOS's bash, after sourcing gce.sh as a driver of the host
        fxdax-test-x does: its status, output and errors."""
        driver = """set -euo pipefail
project=p zone=us-east4-a instance=fxdax-test-x machine=n2-standard-8 purpose=fxdax-test
lifetime=8h limit=6h repo=/laptop/fxdax results=$HOME/results create_flags=()
mkdir -p "$results"
source "$0"
"""
        done = subprocess.run(
            ["/bin/bash", "-c", driver + script, GCE],
            env=self.environment,
            capture_output=True,
            text=True,
        )
        return done.returncode, done.stdout, done.stderr


def host(maker, purpose="fxdax-test"):
    """A host made by the run whose mark is `maker`, for `purpose`."""
    return {
        "zone": "us-east4-a",
        "labels": {"purpose": purpose, "lifecycle": "temporary", "maker": maker},
    }


class Hosts(unittest.TestCase):
    """A host's life: gce_all makes one and deletes it whatever happens, and gce_delete deletes one
    by hand (a driver's `delete`)."""

    def test_a_run_deletes_its_host_when_done(self):
        project = Project({})
        self.assertEqual(
            project.run("gce_all echo ran"),
            (
                0,
                "ran\n",
                "Created fxdax-test-x in us-east4-a.\n"
                "Verified deletion of fxdax-test-x and its boot disk.\n",
            ),
        )
        self.assertEqual(project.state()["instances"], {})

    def test_a_run_deletes_the_host_its_failed_create_made(self):
        project = Project({"create": "ambiguous"})
        self.assertEqual(
            project.run("gce_all echo ran"),
            (
                1,
                "",
                "ERROR: (gcloud.compute.instances.create) the request timed out\n"
                "Verified deletion of fxdax-test-x and its boot disk.\n",
            ),
        )
        self.assertEqual(project.state()["instances"], {})

    def test_a_run_deletes_no_host_another_run_made_at_once(self):
        # Two runs of one name, at once: both found no host, and the other's create came first.
        project = Project({"create": "taken"})
        self.assertEqual(
            project.run("gce_all echo ran"),
            (
                1,
                "",
                "ERROR: (gcloud.compute.instances.create) fxdax-test-x already exists\n"
                "Refusing to delete fxdax-test-x, which another run made.\n",
            ),
        )
        self.assertEqual(project.state()["instances"], {"fxdax-test-x": host("winner")})
        self.assertNotIn("compute instances delete fxdax-test-x", project.calls())

    def test_deleting_by_hand_deletes_a_host_another_run_made(self):
        project = Project({"instances": {"fxdax-test-x": host("earlier")}, "disks": ["fxdax-test-x"]})
        self.assertEqual(
            project.run("gce_locate; gce_delete"),
            (0, "", "Verified deletion of fxdax-test-x and its boot disk.\n"),
        )
        self.assertEqual(project.state(), {"instances": {}, "disks": []})

    def test_a_host_of_another_purpose_is_not_deleted(self):
        other = host("earlier", purpose="someone-else")
        project = Project({"instances": {"fxdax-test-x": other}, "disks": ["fxdax-test-x"]})
        self.assertEqual(
            project.run("status=0; gce_delete || status=$?; echo status $status"),
            (
                0,
                "status 1\n",
                "Refusing to operate on fxdax-test-x, which lacks the fxdax-test labels.\n",
            ),
        )
        self.assertEqual(project.state()["instances"], {"fxdax-test-x": other})

    def test_failed_listings_verify_no_deletion(self):
        made = host("earlier")
        project = Project(
            {
                "instances": {"fxdax-test-x": made},
                "disks": ["fxdax-test-x"],
                "failing": ["instances", "disks"],
            }
        )
        self.assertEqual(
            project.run("status=0; gce_delete || status=$?; echo status $status"),
            (0, "status 1\n", "ERROR: (gcloud.compute.instances.list) listing the instances failed\n"),
        )
        self.assertEqual(project.state()["instances"], {"fxdax-test-x": made})

    def test_a_failed_listing_of_disks_verifies_no_deletion(self):
        project = Project(
            {
                "instances": {"fxdax-test-x": host("earlier")},
                "disks": ["fxdax-test-x"],
                "failing": ["disks"],
            }
        )
        self.assertEqual(
            project.run("status=0; gce_delete || status=$?; echo status $status"),
            (0, "status 1\n", "ERROR: (gcloud.compute.disks.list) listing the disks failed\n"),
        )


class Jobs(unittest.TestCase):
    """What runs on a host, under a driver's `|| status=$?`."""

    def test_a_job_that_did_not_start_is_not_waited_for(self):
        project = Project({"instances": {"fxdax-test-x": host("earlier")}, "ssh": 255})
        self.assertEqual(
            project.run(
                # Waiting would take until the host's end, 8 hours on.
                'sleep() { echo "waited for the job" >&2; exit 3; }\n'
                "status=0; gce_remote_detached job true || status=$?; echo status $status"
            ),
            (0, "status 255\n", ""),
        )
        self.assertEqual(project.calls(), ["compute ssh fxdax-test-x"])

    def test_results_are_not_fetched_from_an_archive_that_failed(self):
        # tar ends with 2 when it could not archive every file.
        project = Project({"instances": {"fxdax-test-x": host("earlier")}, "ssh": 2})
        self.assertEqual(
            project.run("status=0; gce_fetch run-x || status=$?; echo status $status"),
            (0, "status 2\n", ""),
        )
        self.assertEqual(project.calls(), ["compute ssh fxdax-test-x"])


class Migrate(unittest.TestCase):
    """scripts/migrate-gce.sh's hosts and its prefix of the bucket, which it deletes as it ends."""

    def migrate(self, project):
        done = subprocess.run(
            ["/bin/bash", SCRIPTS / "migrate-gce.sh", project.directory / "results"],
            env={
                **project.environment,
                "FXDAX_GCE_PROJECT": "p",
                "FXDAX_MIGRATE_BUCKET": "gs://bucket/prefix",
                "FXDAX_GUEST_KERNEL": "/laptop/vmlinux",
            },
            capture_output=True,
            text=True,
        )
        return done.returncode, done.stderr

    def test_a_migration_deletes_only_what_it_made(self):
        # Another migration's first host, made at once, whose name this one's would have shared.
        project = Project({"create": "taken"})
        returncode, stderr = self.migrate(project)
        created = [call for call in project.calls() if call.startswith("compute instances create")]
        self.assertEqual(len(created), 1, created)
        made = (
            r"compute instances create fxdax-test-migrate-a-(\d{8}-\d{6}-([0-9a-f]{8})) "
            r"--labels=purpose=fxdax-test,lifecycle=temporary,maker=\2"
        )
        # The run's names are its own: they end in its mark, which its hosts carry.
        self.assertRegex(created[0], f"^{made}$")
        stamp = re.fullmatch(made, created[0]).group(1)
        name = f"fxdax-test-migrate-a-{stamp}"
        self.assertEqual(project.state()["instances"], {name: host("winner")})
        self.assertEqual(
            [call for call in project.calls() if not call.startswith("compute instances list")][-3:],
            [
                f"compute instances describe {name}",
                "compute disks list",
                f"storage rm gs://bucket/prefix/run-{stamp}",
            ],
        )
        self.assertEqual(returncode, 1)
        self.assertIn(f"Refusing to delete {name}, which another run made.\n", stderr)

    def test_a_migration_cleans_up_whatever_fails(self):
        project = Project({"failing": ["instances"]})
        returncode, stderr = self.migrate(project)
        self.assertEqual(returncode, 1)
        self.assertRegex(project.calls()[-1], r"^storage rm gs://bucket/prefix/run-\d{8}-\d{6}-[0-9a-f]{8}$")
        self.assertEqual(stderr.splitlines()[-1], f"Results: {project.directory / 'results'}")


if __name__ == "__main__":
    unittest.main()
