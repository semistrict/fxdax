# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/lib/gce.sh, which the GCE drivers source, and of the drivers.

gce_host_command builds the command a driver runs a host script with, and the environment it runs
in: bash builds the command, as a driver does, and another bash, as the host's shell, takes it
apart into the words sudo runs. The hosts' lives, and the jobs run on them, run on a project of
fake_gcloud.py's."""

import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

from script import SCRIPTS

GCE = SCRIPTS / "lib" / "gce.sh"
FAKE_GCLOUD = Path(__file__).resolve().with_name("fake_gcloud.py")
HOST_HOME = "/host/home"
# The variables that stay here (gce_env_local), in its order.
KEPT = [
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
        here = {name: "/laptop/" + name for name in KEPT}
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

    def test_every_variable_kept_here_is_one_the_code_reads(self):
        # A name the code no longer reads, as after a rename, would keep nothing here, and send
        # the new name, which may name this machine's own setup, to the hosts.
        body = re.search(r"\ngce_env_local\(\) \{\n(.*?)\n\}\n", GCE.read_text(), re.S).group(1)
        patterns = re.findall(r"^ *([A-Z0-9_ |]+)\) ;;$", body, re.M)
        self.assertEqual([name for pattern in patterns for name in pattern.split(" | ")], KEPT)
        read = set()
        for directory in ["scripts", "crates", "guest"]:
            for parent, children, files in os.walk(SCRIPTS.parent / directory):
                # Not what was built, nor these tests, whose names are their own.
                children[:] = [
                    child for child in children
                    if child != "target" and Path(parent, child) != SCRIPTS / "tests"
                ]
                for path in (Path(parent, file) for file in files):
                    if path.suffix in [".rs", ".sh", ".py"] and path != GCE:
                        read.update(re.findall(r"\bFXDAX_\w+", path.read_text(errors="replace")))
        self.assertEqual([name for name in KEPT if name not in read], [])

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


class RunNames(unittest.TestCase):
    """Each run's name, its directory on the host and its prefix of a bucket (gce_run_name)."""

    def test_two_drivers_started_at_once_name_their_runs_apart(self):
        # Two drivers, in the same second: each sources gce.sh, with a mark of its own.
        project = Project({})
        status, output, errors = project.run(
            'first=$(gce_run_name); echo "$first"; bash -c \'source "$0"; gce_run_name\' "$0"'
        )
        self.assertEqual((status, errors), (0, ""))
        first, second = output.splitlines()
        name = r"run-\d{8}T\d{6}Z-[0-9a-f]{8}"
        self.assertRegex(first, f"^{name}$")
        self.assertRegex(second, f"^{name}$")
        # Apart whatever second each started in: by their marks.
        self.assertNotEqual(first.rsplit("-", 1)[1], second.rsplit("-", 1)[1])


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

    def test_a_run_makes_its_host_in_the_next_zone_with_room(self):
        project = Project({"zones": ["us-east4-a", "us-east4-b"], "exhausted": ["us-east4-a"]})
        self.assertEqual(
            project.run("gce_all eval 'echo \"ran in $zone\"'"),
            (
                0,
                "ran in us-east4-b\n",
                "us-east4-a has no room for this machine; trying the next zone.\n"
                "Created fxdax-test-x in us-east4-b.\n"
                "Verified deletion of fxdax-test-x and its boot disk.\n",
            ),
        )
        self.assertEqual(project.state()["instances"], {})

    def test_a_run_makes_its_host_in_the_next_region_out_of_quota(self):
        # TASK-97: us-east4 has its zones, and no quota; us-east1 has room.
        project = Project(
            {"zones": ["us-east4-a", "us-east4-b", "us-east1-b"], "quota": ["us-east4"]}
        )
        self.assertEqual(
            project.run("gce_all eval 'echo \"ran in $zone\"'"),
            (
                0,
                "ran in us-east1-b\n",
                "us-east4 is out of quota for this machine; trying the next region.\n"
                "Created fxdax-test-x in us-east1-b.\n"
                "Verified deletion of fxdax-test-x and its boot disk.\n",
            ),
        )
        self.assertEqual(project.state()["instances"], {})

    def test_a_run_deletes_the_host_its_failed_create_made_in_the_next_zone(self):
        project = Project(
            {
                "zones": ["us-east4-a", "us-east4-b"],
                "exhausted": ["us-east4-a"],
                "create": "ambiguous",
            }
        )
        self.assertEqual(
            project.run("gce_all echo ran"),
            (
                1,
                "",
                "us-east4-a has no room for this machine; trying the next zone.\n"
                "ERROR: (gcloud.compute.instances.create) the request timed out\n"
                "Verified deletion of fxdax-test-x and its boot disk.\n",
            ),
        )
        self.assertEqual(project.state()["instances"], {})

    def test_a_failed_listing_of_zones_makes_no_host(self):
        project = Project({"failing": ["zones"]})
        self.assertEqual(
            project.run("status=0; gce_create || status=$?; echo status $status"),
            (0, "status 1\n", "ERROR: (gcloud.compute.zones.list) listing the zones failed\n"),
        )
        self.assertEqual(project.calls(), ["compute zones list"])

    def test_a_run_whose_create_failed_runs_nothing(self):
        # Under `||`, where errexit stops nothing.
        project = Project({"exhausted": ["us-east4-a"]})
        self.assertEqual(
            project.run("(gce_all echo ran) || echo status $?"),
            (
                0,
                "status 1\n",
                "us-east4-a has no room for this machine; trying the next zone.\n"
                "ERROR: (gcloud.compute.instances.create) The zone 'us-east4-a' does not have "
                "enough resources available to fulfill the request.\n"
                "Verified deletion of fxdax-test-x and its boot disk.\n",
            ),
        )

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

    def test_a_host_made_before_hosts_had_a_maker_is_deleted_by_hand_alone(self):
        # gcloud prints a label a host lacks as empty: "fxdax-test<TAB>temporary<TAB>".
        made = {"zone": "us-east4-a", "labels": {"purpose": "fxdax-test", "lifecycle": "temporary"}}
        project = Project({"instances": {"fxdax-test-x": made}, "disks": ["fxdax-test-x"]})
        self.assertEqual(
            project.run("status=0; gce_delete mine || status=$?; echo status $status"),
            (0, "status 1\n", "Refusing to delete fxdax-test-x, which another run made.\n"),
        )
        self.assertEqual(
            project.run("gce_delete"),
            (0, "", "Verified deletion of fxdax-test-x and its boot disk.\n"),
        )
        self.assertEqual(project.state()["instances"], {})

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

    def fetch(self, state):
        """Runs `gce_fetch run-x` on a project as `state` has it, with the host: its status, its
        errors, the files then in the results, and the calls of gcloud."""
        project = Project({"instances": {"fxdax-test-x": host("earlier")}, **state})
        self.results = project.directory / "results"
        returncode, out, err = project.run(
            "status=0; gce_fetch run-x || status=$?; echo status $status"
        )
        self.assertEqual(returncode, 0)
        return out, err, sorted(path.name for path in self.results.iterdir()), project.calls()

    def test_results_are_fetched(self):
        self.assertEqual(
            self.fetch({}),
            (
                "status 0\n",
                "",
                ["fetched", "remote.log"],
                [
                    "compute ssh fxdax-test-x",
                    f"compute scp fxdax-test-x:run-x/results.tar.gz {self.results}/results.tar.gz",
                ],
            ),
        )
        self.assertEqual(
            (self.results / "fetched").read_text(), "fxdax-test-x:run-x/results.tar.gz"
        )

    def test_results_are_fetched_from_an_archive_tar_failed_to_make_whole(self):
        # tar ends with 1 when a file changed as it read it, as the logs of a daemon a killed host
        # script left running do, and with 2 when it could not read one: it archived every other.
        self.assertEqual(
            self.fetch({"ssh": 2}),
            (
                "status 2\n",
                "",
                ["fetched", "remote.log"],
                [
                    "compute ssh fxdax-test-x",
                    f"compute scp fxdax-test-x:run-x/results.tar.gz {self.results}/results.tar.gz",
                ],
            ),
        )

    def test_a_copy_that_failed_fails_the_fetch_after_the_hosts_failure(self):
        failed = "ERROR: (gcloud.compute.scp) copying failed\n"
        self.assertEqual(
            self.fetch({"failing": ["scp"]})[:3], ("status 1\n", failed, ["remote.log"])
        )
        self.assertEqual(
            self.fetch({"ssh": 2, "failing": ["scp"]})[:3], ("status 2\n", failed, ["remote.log"])
        )


class Source(unittest.TestCase):
    """gce_push_source, which copies this machine's working tree to a host, under a driver's
    `|| status=$?`: here a checkout of one file, and Firecracker's of another."""

    def setUp(self):
        self.repo = Path(tempfile.mkdtemp())
        home = tempfile.mkdtemp()
        for tree, files in [
            (self.repo, {".gitignore": "/vendor/\n", "README": "fxdax\n"}),
            (self.repo / "vendor" / "firecracker", {"README": "firecracker\n"}),
        ]:
            tree.mkdir(parents=True, exist_ok=True)
            for name, text in files.items():
                (tree / name).write_text(text)
            git = ["git", "-C", str(tree), "-c", "user.name=t", "-c", "user.email=t@t"]
            for args in [["init", "-q"], ["add", "."], ["commit", "-q", "-m", "a"]]:
                subprocess.run(
                    [*git, *args], env={"PATH": "/usr/bin:/bin", "HOME": home}, check=True
                )

    def push(self, state):
        """Runs `gce_push_source run-x` on a project as `state` has it, with the host: its
        status and errors, the calls of gcloud, its copy's staging directory as STAGING, and the
        project."""
        project = Project({"instances": {"fxdax-test-x": host("earlier")}, **state})
        returncode, out, err = project.run(
            f"repo={shlex.quote(str(self.repo))}\n"
            "status=0; gce_push_source run-x || status=$?; echo status $status"
        )
        self.assertEqual(returncode, 0)
        staging = r"\S*fxdax-gce-source\.\w+"
        for call in project.calls():
            for directory in re.findall(staging, call):
                self.assertFalse(Path(directory).exists())
        return out, err, [re.sub(staging, "STAGING", call) for call in project.calls()], project

    def revision(self, tree):
        return subprocess.run(
            ["git", "-C", str(tree), "rev-parse", "HEAD"], capture_output=True, text=True, check=True
        ).stdout.strip()

    def test_the_source_is_copied_and_recorded(self):
        out, err, calls, project = self.push({})
        self.assertEqual(
            (out, err, calls),
            (
                "status 0\n",
                "",
                [
                    "compute instances describe fxdax-test-x",
                    "compute scp STAGING/source.tar.gz fxdax-test-x:source.tar.gz",
                    "compute ssh fxdax-test-x",
                ],
            ),
        )
        results = project.directory / "results"
        recorded = (results / "source.txt").read_text().splitlines()
        self.assertEqual(
            recorded[:2],
            [
                f"revision {self.revision(self.repo)}",
                f"firecracker {self.revision(self.repo / 'vendor' / 'firecracker')}",
            ],
        )
        self.assertRegex(recorded[2], r"^[0-9a-f]{64}  source\.tar\.gz$")
        self.assertEqual(len(recorded), 3)
        self.assertEqual(
            json.loads((results / "instance.json").read_text()),
            {"name": "fxdax-test-x", "zone": "us-east4-a"},
        )

    def shipped_revision(self, project):
        """The commit the source copied to the host says its builds are of (TASK-111.7)."""
        with tarfile.open(project.directory / "uploaded" / "source.tar.gz") as archive:
            return archive.extractfile("source-revision.txt").read().decode()

    def test_the_source_carries_the_commit_its_builds_report(self):
        project = self.push({})[3]
        self.assertEqual(self.shipped_revision(project), f"{self.revision(self.repo)}\n")

    def test_a_source_changed_since_its_commit_says_so(self):
        (self.repo / "README").write_text("changed\n")
        project = self.push({})[3]
        self.assertEqual(self.shipped_revision(project), f"{self.revision(self.repo)}-dirty\n")

    def test_a_submodule_not_checked_out_is_not_copied(self):
        shutil.rmtree(self.repo / "vendor" / "firecracker")
        (self.repo / "vendor" / "firecracker").mkdir()
        self.assertEqual(
            self.push({})[:3],
            (
                "status 1\n",
                f"vendor/firecracker is not checked out: git -C {self.repo} submodule update "
                "--init vendor/firecracker\n",
                [],
            ),
        )

    def test_a_copy_that_failed_is_not_unpacked(self):
        self.assertEqual(
            self.push({"failing": ["scp"]})[:3],
            (
                "status 1\n",
                "ERROR: (gcloud.compute.scp) copying failed\n",
                [
                    "compute instances describe fxdax-test-x",
                    "compute scp STAGING/source.tar.gz fxdax-test-x:source.tar.gz",
                ],
            ),
        )


class Names(unittest.TestCase):
    """The drivers' default names, of their hosts and their results, which runs at once share none
    of: each ends in its run's mark, which its host carries as its maker."""

    # Each driver's default host name and results directory, before the time, and its own
    # settings.
    DRIVERS = {
        "bench-gce.sh": ("fxdax-bench", "gce-fs-bench", {}),
        "example-gce.sh": ("fxdax-bench-example", "gce-mapbench", {"FXDAX_EXAMPLE": "mapbench"}),
        "gets-gce.sh": ("fxdax-bench-gets", "gce-gets", {"FXDAX_GETS_BUCKET": "gs://bucket/b"}),
        "mutants-gce.sh": ("fxdax-test-mutants", "gce-mutants", {}),
    }

    def test_runs_at_once_have_names_of_their_own(self):
        for driver, (host_name, results, environment) in self.DRIVERS.items():
            with self.subTest(driver):
                project = Project({})
                # The driver in a tree of its own, whose results it names.
                tree = project.directory / "tree"
                (tree / "scripts" / "lib").mkdir(parents=True)
                (tree / "scripts" / driver).symlink_to(SCRIPTS / driver)
                (tree / "scripts" / "lib" / "gce.sh").symlink_to(GCE)
                for _ in range(2):
                    done = subprocess.run(
                        ["/bin/bash", tree / "scripts" / driver, "create"],
                        env={**project.environment, "FXDAX_GCE_PROJECT": "p", **environment},
                        capture_output=True,
                        text=True,
                    )
                    self.assertEqual((done.returncode, done.stdout), (0, ""), done.stderr)
                makers = sorted(
                    made["labels"]["maker"] for made in project.state()["instances"].values()
                )
                self.assertEqual(len(set(makers)), 2, makers)
                self.assertEqual(
                    sorted(
                        re.fullmatch(rf"{host_name}-\d{{8}}-\d{{6}}-([0-9a-f]{{8}})", name).group(1)
                        for name in project.state()["instances"]
                    ),
                    makers,
                )
                kept = sorted((tree / "docs" / "measurements").iterdir())
                self.assertEqual(
                    sorted(
                        re.fullmatch(
                            rf"{results}-\d{{4}}-\d{{2}}-\d{{2}}T\d{{4}}Z-([0-9a-f]{{8}})", path.name
                        ).group(1)
                        for path in kept
                    ),
                    makers,
                )


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
