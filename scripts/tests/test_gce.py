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
        # A driver waits for its job in steps of 30 s: here none.
        (self.directory / "bin" / "sleep").write_text("#!/bin/sh\n")
        (self.directory / "bin" / "sleep").chmod(0o755)
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

    def changes(self):
        """The calls of gcloud that change a host: none of SSH's, nor the listings."""
        return [
            call for call in self.calls()
            if not call.startswith(("compute ssh", "compute instances list", "compute zones list",
                                    "compute instances describe"))
        ]

    def home(self, name):
        """The home of the host `name` (fake_gcloud.py's run_on_host)."""
        home = self.directory / "hosts" / name / "home"
        home.mkdir(parents=True, exist_ok=True)
        return home

    def holder(self, name):
        """Who holds the host `name`'s claim (gce_claim), or None."""
        claim = self.directory / "hosts" / name / "run" / "fxdax-host" / "claim"
        return os.readlink(claim) if claim.is_symlink() else None

    def run(self, script, preamble=""):
        """Runs `script` in macOS's bash, after sourcing gce.sh as a driver of n2-standard-8 hosts,
        its host fxdax-test-x, does, `preamble` before it: its status, output and errors."""
        driver = f"""set -euo pipefail
project=p zone=us-east4-a instance=fxdax-test-x machine=n2-standard-8 limit=6h
repo=/laptop/fxdax results=$HOME/results create_flags=()
{preamble}
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


def host(kind="other", status="RUNNING", zone="us-east4-a"):
    """A kept host of `kind`, in `zone`, as `status` has it."""
    return {
        "zone": zone,
        "labels": {"purpose": "fxdax", "lifecycle": "kept", "kind": kind},
        "status": status,
    }


def kind(preamble=""):
    """The kind of host (gce_kind) a driver of n2-standard-8 hosts needs, as `preamble` sets it."""
    status, out, errors = Project({}).run("gce_kind", preamble)
    assert (status, errors) == (0, ""), errors
    return out.strip()


KIND = kind()
# The host a run of KIND first makes in us-east4-a, and the next.
FIRST = "fxdax-keep-n2-standard-8-us-east4-a-1"
SECOND = "fxdax-keep-n2-standard-8-us-east4-a-2"
# A run's command, which says where it ran.
RAN = "gce_all eval 'echo \"ran on $instance in $zone\"'"


class RunNames(unittest.TestCase):
    """Each run's name, its directory on the host, its prefix of a bucket and the holder of the host
    it claims (gce_run_name)."""

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

    def test_a_run_is_named_for_its_number_and_once(self):
        project = Project({})
        project.environment["FXDAX_GCE_RUN"] = "r1040"
        status, output, errors = project.run("gce_run_name; gce_run_name")
        self.assertEqual((status, errors), (0, ""))
        first, second = output.splitlines()
        self.assertRegex(first, r"^run-r1040-\d{8}T\d{6}Z-[0-9a-f]{8}$")
        self.assertEqual(first, second)


class Hosts(unittest.TestCase):
    """A host's life: a run reuses a kept host of its kind, or makes one if none fits, claims it,
    and stops it whatever happens (gce_all); a driver's delete deletes one by hand."""

    def test_a_run_starts_a_stopped_host_and_stops_it_when_done(self):
        project = Project({"instances": {FIRST: host(KIND, "TERMINATED")}, "disks": [FIRST],
                           "ssh_run": True})
        self.assertEqual(
            project.run(RAN),
            (
                0,
                f"ran on {FIRST} in us-east4-a\n",
                f"Started {FIRST} in us-east4-a.\n"
                f"Reusing {FIRST} in us-east4-a.\n"
                f"Stopped {FIRST} in us-east4-a.\n",
            ),
        )
        # Started with the startup script as it is now, not made: and stopped, not deleted.
        self.assertEqual(
            project.changes(),
            [
                f"compute instances add-metadata {FIRST}",
                f"compute instances start {FIRST}",
                f"compute instances stop {FIRST}",
            ],
        )
        self.assertEqual(project.state()["instances"], {FIRST: host(KIND, "TERMINATED")})
        self.assertEqual(project.state()["disks"], [FIRST])

    def test_a_run_stops_its_host_when_it_fails(self):
        project = Project({"instances": {FIRST: host(KIND, "TERMINATED")}, "disks": [FIRST],
                           "ssh_run": True})
        self.assertEqual(
            project.run("gce_all false"),
            (
                1,
                "",
                f"Started {FIRST} in us-east4-a.\n"
                f"Reusing {FIRST} in us-east4-a.\n"
                f"Stopped {FIRST} in us-east4-a.\n",
            ),
        )
        self.assertEqual(project.state()["instances"], {FIRST: host(KIND, "TERMINATED")})
        self.assertNotIn(f"compute instances delete {FIRST}", project.calls())

    def test_a_run_prefers_a_stopped_host_in_its_zone(self):
        hosts = {
            "fxdax-keep-n2-standard-8-us-west1-b-1": host(KIND, "TERMINATED", "us-west1-b"),
            "fxdax-keep-n2-standard-8-us-east4-b-1": host(KIND, "TERMINATED", "us-east4-b"),
            FIRST: host(KIND, "TERMINATED"),
        }
        project = Project({"instances": hosts, "ssh_run": True})
        self.assertEqual(project.run(RAN)[1], f"ran on {FIRST} in us-east4-a\n")
        project = Project({"instances": {k: v for k, v in hosts.items() if k != FIRST},
                           "ssh_run": True})
        # Else one in its region.
        self.assertEqual(
            project.run(RAN)[1], "ran on fxdax-keep-n2-standard-8-us-east4-b-1 in us-east4-b\n"
        )

    def test_a_run_makes_a_host_only_when_none_fits(self):
        # A host of another kind (another machine's, or one with a service account), of the same
        # name but for its number: it fits no run of KIND.
        other = host("0123456789ab", "TERMINATED")
        project = Project({"instances": {FIRST: other}, "disks": [FIRST], "ssh_run": True})
        self.assertEqual(
            project.run(RAN),
            (
                0,
                f"ran on {SECOND} in us-east4-a\n",
                f"Created {SECOND} in us-east4-a.\nStopped {SECOND} in us-east4-a.\n",
            ),
        )
        self.assertEqual(
            project.changes(),
            [
                f"compute instances create {SECOND} --labels=purpose=fxdax,lifecycle=kept,kind={KIND}",
                f"compute instances stop {SECOND}",
            ],
        )
        self.assertEqual(
            project.state()["instances"], {FIRST: other, SECOND: host(KIND, "TERMINATED")}
        )

    def test_a_host_another_run_holds_is_not_shared(self):
        # One run holds the host it made; a second, at once, makes another rather than share it.
        project = Project({"ssh_run": True})
        self.assertEqual(
            project.run('gce_acquire held-first; echo "$instance"'),
            (0, f"{FIRST}\n", f"Created {FIRST} in us-east4-a.\n"),
        )
        self.assertEqual(
            project.run(RAN),
            (
                0,
                f"ran on {SECOND} in us-east4-a\n",
                f"{FIRST} is another run's; trying the next host.\n"
                f"Created {SECOND} in us-east4-a.\n"
                f"Stopped {SECOND} in us-east4-a.\n",
            ),
        )
        # The first's host runs on, its claim the first's.
        self.assertEqual(project.state()["instances"][FIRST]["status"], "RUNNING")
        self.assertEqual(project.holder(FIRST), "held-first")
        self.assertNotIn(f"compute instances stop {FIRST}", project.calls())
        # Its stop releases it: the next run reuses it.
        self.assertEqual(project.run(f"instance={FIRST}; gce_stop")[0], 0)
        self.assertEqual(project.run(RAN)[1], f"ran on {FIRST} in us-east4-a\n")

    def test_a_run_claims_no_host_it_already_holds(self):
        # migrate-gce.sh's two hosts, claimed for one run: not one host twice.
        project = Project({"instances": {FIRST: host(KIND, "TERMINATED")}, "ssh_run": True})
        self.assertEqual(
            project.run('gce_acquire "$gce_run"; gce_acquire "$gce_run"; echo "$instance"')[:2],
            (0, f"{SECOND}\n"),
        )

    def test_an_idle_running_host_is_reused(self):
        # Running, as one whose driver was killed is, and no one's.
        project = Project({"instances": {FIRST: host(KIND)}, "ssh_run": True})
        self.assertEqual(
            project.run(RAN),
            (
                0,
                f"ran on {FIRST} in us-east4-a\n",
                f"Reusing {FIRST} in us-east4-a.\nStopped {FIRST} in us-east4-a.\n",
            ),
        )

    def test_a_stopped_host_that_cannot_start_is_passed_over(self):
        # Its zone has no room for it now: the run makes a host in the next zone.
        project = Project(
            {
                "instances": {FIRST: host(KIND, "TERMINATED")},
                "zones": ["us-east4-a", "us-east4-b"],
                "exhausted": ["us-east4-a"],
                "ssh_run": True,
            }
        )
        status, out, errors = project.run(RAN)
        self.assertEqual(
            (status, out),
            (0, "ran on fxdax-keep-n2-standard-8-us-east4-b-1 in us-east4-b\n"),
        )
        self.assertEqual(
            errors,
            "ERROR: (gcloud.compute.instances.start) The zone 'us-east4-a' does not have enough "
            "resources available to fulfill the request.\n"
            f"{FIRST} did not start; trying the next host.\n"
            "us-east4-a has no room for this machine; trying the next zone.\n"
            "Created fxdax-keep-n2-standard-8-us-east4-b-1 in us-east4-b.\n"
            "Stopped fxdax-keep-n2-standard-8-us-east4-b-1 in us-east4-b.\n",
        )

    def test_a_run_makes_its_host_in_its_own_zone_first(self):
        # Its zone is us-east4-b; the host it passed over, which would not start, us-east4-a's.
        project = Project(
            {
                "instances": {FIRST: host(KIND, "TERMINATED")},
                "zones": ["us-east4-a", "us-east4-b"],
                "exhausted": ["us-east4-a"],
                "ssh_run": True,
            }
        )
        status, out, errors = project.run(RAN, preamble="zone=us-east4-b")
        self.assertEqual(
            (status, out), (0, "ran on fxdax-keep-n2-standard-8-us-east4-b-1 in us-east4-b\n")
        )
        self.assertNotIn("us-east4-a has no room", errors)

    def test_a_run_takes_the_host_its_failed_create_made(self):
        project = Project({"create": "ambiguous", "ssh_run": True})
        self.assertEqual(
            project.run(RAN),
            (
                0,
                f"ran on {FIRST} in us-east4-a\n",
                "ERROR: (gcloud.compute.instances.create) the request timed out\n"
                f"Stopped {FIRST} in us-east4-a.\n",
            ),
        )
        self.assertEqual(project.state()["instances"], {FIRST: host(KIND, "TERMINATED")})

    def test_two_runs_making_a_host_at_once_name_theirs_apart(self):
        # Both found no host, and the other's create of the first name came first.
        project = Project({"create": ["taken", "made"], "ssh_run": True})
        self.assertEqual(
            project.run(RAN),
            (
                0,
                f"ran on {SECOND} in us-east4-a\n",
                f"Another run made {FIRST} as this one did; trying the next name.\n"
                f"Created {SECOND} in us-east4-a.\n"
                f"Stopped {SECOND} in us-east4-a.\n",
            ),
        )
        self.assertEqual(project.state()["instances"][FIRST]["labels"]["maker"], "winner")
        self.assertNotIn(f"compute instances stop {FIRST}", project.calls())

    def test_a_run_makes_its_host_in_the_next_zone_with_room(self):
        project = Project(
            {"zones": ["us-east4-a", "us-east4-b"], "exhausted": ["us-east4-a"], "ssh_run": True}
        )
        self.assertEqual(
            project.run(RAN),
            (
                0,
                "ran on fxdax-keep-n2-standard-8-us-east4-b-1 in us-east4-b\n",
                "us-east4-a has no room for this machine; trying the next zone.\n"
                "Created fxdax-keep-n2-standard-8-us-east4-b-1 in us-east4-b.\n"
                "Stopped fxdax-keep-n2-standard-8-us-east4-b-1 in us-east4-b.\n",
            ),
        )

    def test_a_run_makes_its_host_in_the_next_region_out_of_quota(self):
        # TASK-97: us-east4 has its zones, and no quota; us-east1 has room.
        project = Project(
            {"zones": ["us-east4-a", "us-east4-b", "us-east1-b"], "quota": ["us-east4"],
             "ssh_run": True}
        )
        self.assertEqual(
            project.run(RAN),
            (
                0,
                "ran on fxdax-keep-n2-standard-8-us-east1-b-1 in us-east1-b\n",
                "us-east4 is out of quota for this machine; trying the next region.\n"
                "Created fxdax-keep-n2-standard-8-us-east1-b-1 in us-east1-b.\n"
                "Stopped fxdax-keep-n2-standard-8-us-east1-b-1 in us-east1-b.\n",
            ),
        )

    def test_a_failed_listing_of_zones_makes_no_host(self):
        project = Project({"failing": ["zones"]})
        self.assertEqual(
            project.run("status=0; gce_create || status=$?; echo status $status"),
            (0, "status 1\n", "ERROR: (gcloud.compute.zones.list) listing the zones failed\n"),
        )
        self.assertEqual(project.calls(), ["compute zones list"])

    def test_a_run_that_has_no_host_runs_nothing(self):
        # Under `||`, where errexit stops nothing.
        project = Project({"exhausted": ["us-east4-a"], "ssh_run": True})
        self.assertEqual(
            project.run("(gce_all echo ran) || echo status $?"),
            (
                0,
                "status 1\n",
                "us-east4-a has no room for this machine; trying the next zone.\n"
                "ERROR: (gcloud.compute.instances.create) The zone 'us-east4-a' does not have "
                "enough resources available to fulfill the request.\n",
            ),
        )
        self.assertEqual(project.state().get("instances", {}), {})

    def test_a_failed_listing_of_hosts_runs_nothing(self):
        project = Project({"failing": ["instances"], "ssh_run": True})
        self.assertEqual(
            project.run("(gce_all echo ran) || echo status $?"),
            (
                0,
                "status 1\n",
                "ERROR: (gcloud.compute.instances.list) listing the instances failed\n",
            ),
        )
        self.assertEqual(project.changes(), [])

    def test_a_host_a_create_holds_is_run_on_until_its_stop(self):
        project = Project({"ssh_run": True})
        driver = 'run() { echo "ran on $instance"; }\n'
        self.assertEqual(
            project.run(driver + "gce_main create"),
            (
                0,
                f"{FIRST}\n",
                f"Created {FIRST} in us-east4-a.\n"
                f"Holding {FIRST} in us-east4-a: run on it with `run {FIRST}`, stop it with "
                f"`stop {FIRST}`.\n",
            ),
        )
        for _ in range(2):
            self.assertEqual(project.run(driver + f"gce_main run {FIRST}"), (0, f"ran on {FIRST}\n", ""))
        self.assertEqual(project.state()["instances"][FIRST]["status"], "RUNNING")
        self.assertEqual(
            project.run(driver + f"gce_main stop {FIRST}"), (0, "", f"Stopped {FIRST} in us-east4-a.\n")
        )
        self.assertEqual(project.state()["instances"], {FIRST: host(KIND, "TERMINATED")})

    def test_a_run_named_runs_on_no_host_but_a_held_one(self):
        # Another run's, which gce_all claimed: and a stopped one, which no one holds.
        project = Project({"instances": {FIRST: host(KIND, "TERMINATED")}, "ssh_run": True})
        driver = 'run() { echo "ran on $instance"; }\n'
        self.assertEqual(project.run(f'gce_acquire "$gce_run"')[0], 0)
        status, out, errors = project.run(driver + f"gce_main run {FIRST}")
        self.assertEqual((status, out), (1, ""))
        self.assertRegex(
            errors,
            rf"^{FIRST} is not held for runs \(run-\d{{8}}T\d{{6}}Z-[0-9a-f]{{8}}\): hold a host "
            r"with create\.\n$",
        )
        self.assertEqual(project.run(f"instance={FIRST}; gce_stop")[0], 0)
        self.assertEqual(
            project.run(driver + f"gce_main run {FIRST}"),
            (
                1,
                "",
                f"{FIRST} is not held for runs (not running, or unclaimed): hold a host with create.\n",
            ),
        )

    def test_deleting_by_hand_deletes_a_kept_host(self):
        project = Project({"instances": {FIRST: host(KIND, "TERMINATED")}, "disks": [FIRST]})
        self.assertEqual(
            project.run(f"gce_main delete {FIRST}"),
            (0, "", f"Verified deletion of {FIRST} and its boot disk.\n"),
        )
        self.assertEqual(project.state(), {"instances": {}, "disks": []})

    def test_a_host_that_is_not_kept_is_not_deleted(self):
        other = {"zone": "us-east4-a", "labels": {"purpose": "someone-else", "lifecycle": "kept"}}
        project = Project({"instances": {"fxdax-test-x": other}, "disks": ["fxdax-test-x"]})
        self.assertEqual(
            project.run("status=0; gce_delete || status=$?; echo status $status"),
            (
                0,
                "status 1\n",
                "Refusing to operate on fxdax-test-x, which lacks the labels of a kept host.\n",
            ),
        )
        self.assertEqual(project.state()["instances"], {"fxdax-test-x": other})

    def test_failed_listings_verify_no_deletion(self):
        made = host(KIND)
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
                "instances": {"fxdax-test-x": host(KIND)},
                "disks": ["fxdax-test-x"],
                "failing": ["disks"],
            }
        )
        self.assertEqual(
            project.run("status=0; gce_delete || status=$?; echo status $status"),
            (0, "status 1\n", "ERROR: (gcloud.compute.disks.list) listing the disks failed\n"),
        )


class Resume(unittest.TestCase):
    """test-gce.sh's resume, which waits for a run whose waiter was stopped, on its kept host, and
    fetches its results."""

    RUN = "run-r1040-20261108T120000Z-0123abcd"

    def resume(self, state, job, holder=None):
        """Runs `test-gce.sh resume FIRST RESULTS RUN` on a project as `state` has it, the host's
        home holding RUN's job as `job` has it (its files' contents), and RUN's results, and its
        claim `holder`'s, if given: its status, its errors and the project."""
        project = Project({"ssh_run": True, **state})
        home = project.home(FIRST)
        if holder:
            claim = project.directory / "hosts" / FIRST / "run" / "fxdax-host"
            claim.mkdir(parents=True)
            (claim / "ready").touch()
            os.symlink(holder, claim / "claim")
        (home / f"{self.RUN}.job").mkdir()
        for name, text in job.items():
            (home / f"{self.RUN}.job" / name).write_text(text)
        (home / self.RUN / "results").mkdir(parents=True)
        (home / self.RUN / "results" / "host.txt").write_text("a host\n")
        done = subprocess.run(
            ["/bin/bash", SCRIPTS / "test-gce.sh", "resume", FIRST, project.directory / "results",
             self.RUN],
            env={**project.environment, "FXDAX_GCE_PROJECT": "p"},
            capture_output=True,
            text=True,
        )
        return done.returncode, done.stderr, project

    def test_a_run_on_a_stopped_host_is_resumed_and_the_host_stopped(self):
        # The run's job ended, its waiter gone, and the host powered off, idle.
        status, errors, project = self.resume(
            {"instances": {FIRST: host(KIND, "TERMINATED")}}, {"status": "0\n", "log": "ran\n"}
        )
        results = project.directory / "results"
        self.assertEqual(
            (status, errors),
            (
                0,
                f"Results: {results}\nStarted {FIRST} in us-east4-a.\nran\n"
                f"Results: {results}\nStopped {FIRST} in us-east4-a.\n",
            ),
        )
        self.assertEqual(
            project.changes(),
            [
                f"compute instances add-metadata {FIRST}",
                f"compute instances start {FIRST}",
                f"compute scp {FIRST}:{self.RUN}/results.tar.gz {results}/results.tar.gz",
                f"compute instances stop {FIRST}",
            ],
        )
        self.assertEqual(project.state()["instances"][FIRST]["status"], "TERMINATED")

    def test_a_run_on_a_host_another_run_holds_is_not_resumed(self):
        other = "run-20261108T130000Z-ffffffff"
        status, errors, project = self.resume(
            {"instances": {FIRST: host(KIND)}}, {"status": "0\n", "log": "ran\n"}, holder=other
        )
        self.assertEqual(
            (status, errors.splitlines()[-1]),
            (1, f"{FIRST} is another run's ({other}), not {self.RUN}'s."),
        )
        # Not this run's to stop.
        self.assertEqual(project.state()["instances"][FIRST]["status"], "RUNNING")

    def test_a_run_on_a_host_a_create_holds_is_resumed_and_the_host_kept(self):
        status, errors, project = self.resume(
            {"instances": {FIRST: host(KIND)}}, {"status": "0\n", "log": "ran\n"},
            holder="held-0123abcd",
        )
        self.assertEqual((status, errors.splitlines()[-1]), (0, f"Results: {project.directory / 'results'}"))
        self.assertEqual(project.state()["instances"][FIRST]["status"], "RUNNING")

    def test_a_job_its_hosts_stop_ended_is_lost(self):
        # A pid no process has: the job's process went with the host's power, writing no status.
        status, errors, project = self.resume(
            {"instances": {FIRST: host(KIND, "TERMINATED")}}, {"pid": "4194303\n", "log": "half\n"}
        )
        self.assertEqual(status, 1)
        self.assertIn(
            f"{self.RUN} on {FIRST} is lost: its process ended without its status.\nhalf\n", errors
        )
        self.assertEqual(project.state()["instances"][FIRST]["status"], "TERMINATED")


class Jobs(unittest.TestCase):
    """What runs on a host, under a driver's `|| status=$?`."""

    def test_a_job_that_did_not_start_is_not_waited_for(self):
        project = Project({"instances": {"fxdax-test-x": host(KIND)}, "ssh": 255})
        self.assertEqual(
            project.run(
                # Waiting would take until the host's end, 8 hours on.
                'sleep() { echo "waited for the job" >&2; exit 3; }\n'
                "status=0; gce_remote_detached job true || status=$?; echo status $status"
            ),
            (0, "status 255\n", ""),
        )
        # The start, and a fresh connection's look for its pid, which finds none.
        self.assertEqual(project.calls(), ["compute ssh fxdax-test-x"] * 2)

    def started(self, state):
        """Runs `gce_remote_detached job`, its job echoing and failing with 3, on a project whose
        host runs what SSH asks (fake_gcloud.py's ssh_run), as `state` adds to it: its status,
        output and errors, and whether the job's pid was kept on the host."""
        project = Project(
            {"instances": {"fxdax-test-x": host(KIND)}, "ssh_run": True, **state}
        )
        done = project.run(
            "sleep() { :; }\n"
            "status=0; gce_remote_detached job 'echo ran; (exit 3)' || status=$?; echo status $status"
        )
        return done, (project.home("fxdax-test-x") / "job.job" / "pid").is_file()

    def test_a_job_keeps_its_pid_and_is_waited_for(self):
        # TASK-235: its pid kept as it is launched, which a dropped start looks for.
        self.assertEqual(self.started({}), ((0, "ran\nstatus 3\n", ""), True))

    def test_a_job_whose_starting_connection_dropped_is_waited_for(self):
        # TASK-235: the start's SSH fails once the job runs, as r942's did: a fresh connection
        # finds its pid, and it is waited for.
        self.assertEqual(
            self.started({"ssh_calls": [[255, ""]]}), ((0, "ran\nstatus 3\n", ""), True)
        )

    def fetch(self, state):
        """Runs `gce_fetch run-x` on a project as `state` has it, with the host: its status, its
        errors, the files then in the results, and the calls of gcloud."""
        project = Project({"instances": {"fxdax-test-x": host(KIND)}, **state})
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
        project = Project({"instances": {"fxdax-test-x": host(KIND)}, **state})
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
    """The drivers' hosts and results: two runs at once hold two hosts, and keep their results
    apart, each's directory ending in its run's mark."""

    # Each driver's results directory, before the time, its machine, and its own settings.
    DRIVERS = {
        "bench-gce.sh": ("gce-fs-bench", "c3-standard-8", {}),
        "example-gce.sh": ("gce-mapbench", "n2-standard-8", {"FXDAX_EXAMPLE": "mapbench"}),
        "gets-gce.sh": ("gce-gets", "c3-standard-8", {"FXDAX_GETS_BUCKET": "gs://bucket/b"}),
        "mutants-gce.sh": ("gce-mutants", "n2-standard-4", {}),
    }

    def test_runs_at_once_have_hosts_and_names_of_their_own(self):
        for driver, (results, machine, environment) in self.DRIVERS.items():
            with self.subTest(driver):
                project = Project({"ssh_run": True})
                # The driver in a tree of its own, whose results it names.
                tree = project.directory / "tree"
                (tree / "scripts" / "lib").mkdir(parents=True)
                (tree / "scripts" / driver).symlink_to(SCRIPTS / driver)
                (tree / "scripts" / "lib" / "gce.sh").symlink_to(GCE)
                held = []
                for _ in range(2):
                    done = subprocess.run(
                        ["/bin/bash", tree / "scripts" / driver, "create"],
                        env={**project.environment, "FXDAX_GCE_PROJECT": "p", **environment},
                        capture_output=True,
                        text=True,
                    )
                    self.assertEqual(done.returncode, 0, done.stderr)
                    held.append(done.stdout)
                self.assertEqual(
                    held,
                    [f"fxdax-keep-{machine}-us-east4-a-1\n", f"fxdax-keep-{machine}-us-east4-a-2\n"],
                )
                kept = sorted((tree / "docs" / "measurements").iterdir())
                marks = {
                    re.fullmatch(rf"{results}-\d{{4}}-\d{{2}}-\d{{2}}T\d{{4}}Z-([0-9a-f]{{8}})", path.name)
                    .group(1)
                    for path in kept
                }
                self.assertEqual(len(marks), 2, kept)


class Migrate(unittest.TestCase):
    """scripts/migrate-gce.sh's two hosts and its prefix of the bucket, which it stops and deletes
    as it ends."""

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

    def test_a_migration_holds_two_hosts_and_stops_what_it_held(self):
        # One kept host of its kind, which its first side takes: its second takes not that one
        # again, and finds no quota for another.
        first = "fxdax-keep-c3-standard-8-us-east4-a-1"
        fits = kind("machine=c3-standard-8 FXDAX_GCE_STORAGE=1")
        project = Project(
            {"instances": {first: host(fits, "TERMINATED")}, "quota": ["us-east4"], "ssh_run": True}
        )
        project.environment["FXDAX_GCE_REGIONS"] = "us-east4"
        returncode, stderr = self.migrate(project)
        self.assertEqual(returncode, 1, stderr)
        self.assertIn(f"{first} is another run's; trying the next host.\n", stderr)
        self.assertEqual(project.state()["instances"], {first: host(fits, "TERMINATED")})
        changes = project.changes()
        self.assertEqual(
            [re.sub(r"run-\d{8}T\d{6}Z-[0-9a-f]{8}$", "RUN", change) for change in changes],
            [
                f"compute instances add-metadata {first}",
                f"compute instances start {first}",
                "compute instances create fxdax-keep-c3-standard-8-us-east4-a-2 "
                f"--labels=purpose=fxdax,lifecycle=kept,kind={fits}",
                f"compute instances stop {first}",
                "storage rm gs://bucket/prefix/RUN",
            ],
        )

    def test_a_migration_cleans_up_whatever_fails(self):
        project = Project({"failing": ["instances"]})
        returncode, stderr = self.migrate(project)
        self.assertEqual(returncode, 1)
        self.assertRegex(project.calls()[-1], r"^storage rm gs://bucket/prefix/run-\d{8}T\d{6}Z-[0-9a-f]{8}$")
        self.assertEqual(stderr.splitlines()[-1], f"Results: {project.directory / 'results'}")


if __name__ == "__main__":
    unittest.main()
