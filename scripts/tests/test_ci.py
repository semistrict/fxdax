# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of .github/workflows/ci.yml's test job: each part installs what its tests run, and no part
what only another's does. Every part fetching Coccinelle and its OCaml, three quarters of the
downloads, a slow mirror took 40 minutes of the workspace part's 90, which its tests then outran
(run 42089880804, TASK-172)."""

import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
CI = ROOT / ".github" / "workflows" / "ci.yml"
APT_INSTALL = ROOT / ".github" / "actions" / "apt-install" / "action.yml"
# GitHub's ubuntu-24.04 image's /etc/apt/apt-mirrors.txt (probe run 43052221835).
IMAGE_MIRRORS = ("http://azure.archive.ubuntu.com/ubuntu/\tpriority:1\n"
                 "https://archive.ubuntu.com/ubuntu/\tpriority:2\n"
                 "https://security.ubuntu.com/ubuntu/\tpriority:3\n")
UBUNTU_COM = ["azure.archive.ubuntu.com", "archive.ubuntu.com", "security.ubuntu.com"]
FALLBACKS = ["mirrors.edge.kernel.org", "mirrors.mit.edu"]
TEST_SH = ROOT / "scripts" / "test.sh"
# The packages only some parts' tests run, and the parts: Coccinelle's spatch, which the scripts'
# tests run (test_cocci_guest.py); mkfs.xfs, which the guest tests' own do; libclang, which the
# VMM's build needs (userfaultfd-sys's bindgen).
OWN = {"coccinelle": {"clippy scripts"}, "xfsprogs": {"guest"}, "libclang-dev": {"firecracker"}}


def job():
    """The test job's text."""
    text = CI.read_text()
    return re.search(r"^  test:\n(.*?)^  \S", text, re.MULTILINE | re.DOTALL).group(1)


def workflow_job(text, name):
    """The text of the job `name` of a workflow's `text`."""
    match = re.search(rf"^  {re.escape(name)}:\n(.*?)(?=^  \S|\Z)", text, re.MULTILINE | re.DOTALL)
    return match.group(1)


def parts():
    """The parts the job's matrix runs."""
    listed = re.search(r"^        parts: \[(.*)\]$", job(), re.MULTILINE).group(1)
    return [part.strip() for part in listed.split(",")]


def packages():
    """Each part's packages: those every part installs, and its own (matrix.include's)."""
    install = re.search(r"uses: \./\.github/actions/apt-install\n *with:\n *packages: (.*)$", job(),
                        re.MULTILINE)
    common = set(install.group(1).split()) - {"${{", "matrix.packages", "}}"}
    own = dict(re.findall(r"^          - parts: (.+)\n            packages: (.+)$", job(),
                          re.MULTILINE))
    return {part: common | set(own.get(part, "").split()) for part in parts()}


class Ci(unittest.TestCase):
    def test_each_part_installs_its_own_tools_alone(self):
        installed = packages()
        self.assertEqual(sorted(installed), sorted(["workspace", "sim", "shuttle",
                                                    "clippy scripts", "guest", "go",
                                                    "firecracker"]))
        for package, users in OWN.items():
            self.assertEqual({part for part, has in installed.items() if package in has}, users,
                             package)

    def test_every_part_installs_what_the_builds_need(self):
        for part, has in packages().items():
            self.assertLessEqual({"clang", "elfutils", "sqlite3", "libseccomp-dev"}, has, part)

    def test_no_build_needs_protoc_or_libclang(self):
        """What lets the job install neither but in the firecracker part: the gRPC API compiled by
        protox, not protoc, and no bindgen, which loads libclang, in fxdax's workspace or the guest
        tests'. One that comes needs them back. The VMM's has it, its part's own libclang's why."""
        self.assertIn("protox::compile", (ROOT / "crates/fxdax/build.rs").read_text())
        for lock in [ROOT / "Cargo.lock", ROOT / "guest/tests/Cargo.lock"]:
            self.assertNotIn('name = "bindgen"', lock.read_text(), lock)
        self.assertIn('name = "bindgen"', (ROOT / "vendor/firecracker.lock").read_text())

    def test_each_part_is_one_test_sh_runs(self):
        """Each part of the matrix is one scripts/test.sh runs (`part NAME`): a part it hasn't runs
        nothing, and passes."""
        script = TEST_SH.read_text()
        for part in parts():
            for name in part.split():
                self.assertIn(f"if part {name}; then", script, part)

    def test_the_vmms_own_tests_run_with_a_bucket(self):
        """TASK-220: the firecracker part runs the VMM's extent device's tests and its vCPUs'
        exits', from vendor/firecracker as fxdax builds it (scripts/firecracker-cargo.sh, TASK-269),
        each extent test's daemon a host's, its stores' branches in a bucket in memory: with none
        named, every store's opening fails EIO, which 8 of its 21 did, run by nothing but
        scripts/test-e2e.sh on GCE. And the cache keeps its builds, in target/firecracker."""
        script = TEST_SH.read_text()
        body = re.search(r"^if part firecracker; then\n(.*?)^fi$", script,
                         re.MULTILINE | re.DOTALL).group(1)
        self.assertIn('"$root/scripts/firecracker-cargo.sh" nextest run -p vmm --lib', body)
        self.assertIn("FXDAX_EXTENTD_REMOTE=memory:///", body)
        self.assertIn("FXDAX_EXTENTD_TOKEN=1", body)
        self.assertIn("--features fxdax", body)
        self.assertIn("test(/extent::/)", body)
        self.assertIn("            target\n", job())
        self.assertIn("'vendor/firecracker.lock'", job())
        e2e = (ROOT / "scripts" / "test-e2e.sh").read_text()
        self.assertIn('FXDAX_TEST_PARTS="guest firecracker" "$root/scripts/test.sh"', e2e)

    def test_guest_sim_runs_whole_when_what_it_tests_changed_and_nightly(self):
        """The user's decision (2026-11-08): guest/sim's jobs, keeping their names, run whole in CI
        only when what they test changed since the base, which scripts/guest-sim-scope.py says
        from the base firecracker-forward compares with: else 200 seeds, and a twelfth of the
        sweep's calls, rotating with the run's number; the mutants checked either way. The nightly
        workflow runs both whole."""
        text = CI.read_text()
        base = re.search(r"^          BASE: (.*)$", workflow_job(text, "firecracker-forward"),
                         re.MULTILINE).group(1)
        for name in ["guest-sim", "guest-sim-sweep"]:
            body = workflow_job(text, name)
            self.assertIn("fetch-depth: 0", body, name)
            self.assertIn(f"          BASE: {base}\n", body, name)
            self.assertIn("FXDAX_GUEST_SIM: ${{ inputs.guest-sim }}", body, name)
            self.assertIn('run: echo "scope=$(scripts/guest-sim-scope.py "$BASE")"', body, name)
        seeds = workflow_job(text, "guest-sim")
        self.assertIn("guest/sim/mutants.sh --check", seeds)
        self.assertIn("SEEDS: ${{ steps.scope.outputs.scope == 'full' && 3000 || 200 }}", seeds)
        self.assertIn('guest/sim/zloop.sh -s "$SEEDS"', seeds)
        sweep = workflow_job(text, "guest-sim-sweep")
        self.assertIn('guest/sim/sweep.sh -s "$GITHUB_RUN_NUMBER/12"', sweep)
        self.assertIn('guest/sim/sweep.sh -d "$BUILD/sweep"', sweep)
        nightly = (ROOT / ".github" / "workflows" / "nightly.yml").read_text()
        self.assertIn('guest/sim/zloop.sh -s 3000 -d "$BUILD/logs"',
                      workflow_job(nightly, "guest-sim"))
        self.assertIn('guest/sim/sweep.sh -d "$BUILD/sweep"',
                      workflow_job(nightly, "guest-sim-sweep"))

    def test_no_comment_says_vms_need_gce(self):
        """TASK-284: VMs boot on GitHub's runners, which have KVM; no GCP anywhere (the user,
        2026-11-11). ci.yml's header said they ran on GCE."""
        for name in ["ci.yml", "nightly.yml", "sandboxes.yml"]:
            text = (ROOT / ".github" / "workflows" / name).read_text()
            self.assertNotRegex(text, r"(?i)\bgce\b|gcloud|google", name)
        self.assertIn("No guest\n# kernels nor VMs, which boot on GitHub's runners too",
                      CI.read_text())

    def test_macos_runs_what_compiles_for_macos(self):
        """TASK-149.20.2: the macos job runs scripts/test.sh's workspace and Shuttle parts on a
        macOS runner, a part a job, every failure: what fxdax compiles for macOS alone (physmem's
        physical map, extent_store's syncs, extent_remote's cache, fxdax's storage I/O) ran on
        laptops alone."""
        text = CI.read_text()
        macos = re.search(r"^  macos:\n(.*?)^  \S", text, re.MULTILINE | re.DOTALL).group(1)
        self.assertIn("runs-on: macos-15", macos)
        self.assertIn("parts: [workspace, shuttle]", macos)
        self.assertIn("FXDAX_TEST_PARTS: ${{ matrix.parts }}", macos)
        self.assertIn("run: scripts/test.sh --no-fail-fast", macos)
        self.assertIn("submodules: true", macos)


class Nightly(unittest.TestCase):
    """TASK-284: the nightly's guest suites, on GitHub's runners, as production runs them, with no
    GCP: every night they failed at GCP's authentication, its secrets never set, so they never ran.
    """

    def setUp(self):
        self.text = (ROOT / ".github" / "workflows" / "nightly.yml").read_text()

    def test_it_uses_no_gcp(self):
        for gcp in ["google-github-actions", "gcloud", "GCE", "GCP", "gs://", "id-token",
                    "test-gce.sh", "secrets."]:
            self.assertNotIn(gcp, self.text)

    def test_the_suite_is_built_once_for_its_parts(self):
        build = workflow_job(self.text, "guest-build")
        self.assertIn("uses: ./.github/actions/guest-host", build)
        self.assertIn("uses: ./.github/actions/guest-kernel", build)
        self.assertIn('FXDAX_E2E_BUILD_ONLY="$HOME/guest/artifacts" scripts/test-e2e.sh', build)
        self.assertIn("submodules: true", build)
        self.assertIn("name: guest-artifacts", build)

    def test_its_parts_run_the_suite_as_production_runs_it(self):
        suites = workflow_job(self.text, "guest-suites")
        self.assertIn("needs: guest-build", suites)
        parts = re.search(r"^        part: \[(.*)\]$", suites, re.MULTILINE).group(1)
        count = len(parts.split(","))
        self.assertEqual([int(part) for part in parts.split(",")], list(range(1, count + 1)))
        self.assertIn("SHARD: ${{ matrix.part }}/${{ strategy.job-total }}", suites)
        self.assertIn("fail-fast: false", suites)
        # Where the jailed VMs' user reaches, binding their sockets beside the cases' logs: in the
        # runner's home, its user's alone, every case with one failed, EACCES (run 42981086953).
        self.assertIn("      WORK: /mnt/nightly\n", suites)
        self.assertIn('chmod 1777 "$WORK/tmp"', suites)
        self.assertIn('"TMPDIR=$WORK/tmp"', suites)
        self.assertNotIn("$HOME/nightly", suites)
        run = re.search(r"- name: The guest suites, part .*?\n(.*?)\n      # ", suites,
                        re.DOTALL).group(1)
        for want in ["FXDAX_FIRECRACKER_JAIL=1", "FXDAX_FIRECRACKER_SECCOMP=real",
                     '"FXDAX_E2E_PREBUILT=$WORK/artifacts"', '"FXDAX_GUEST_KERNEL=$WORK/vmlinux"',
                     '"FXDAX_E2E_RESULTS=$WORK/results"', "sudo --preserve-env",
                     'scripts/test-e2e-rustfs.sh --tag nightly --shard "$SHARD"']:
            self.assertIn(want, run)
        # Its results uploaded however it ends, the run's step stopped in time for that.
        step = int(re.search(r"^        timeout-minutes: (\d+)$", suites, re.MULTILINE).group(1))
        job = int(re.search(r"^    timeout-minutes: (\d+)$", suites, re.MULTILINE).group(1))
        self.assertLess(step, job)
        self.assertLessEqual(job, 360)
        self.assertIn("      - uses: actions/upload-artifact@v7\n        if: always()\n", suites)
        self.assertIn("name: guest-suites-${{ matrix.part }}", suites)

    def test_the_bucket_is_rustfs_on_the_runner(self):
        script = (ROOT / "scripts" / "test-e2e-rustfs.sh").read_text()
        self.assertIn('FXDAX_EXTENTD_REMOTE=s3://fxdax/e2e "$root/scripts/test-e2e.sh" "$@"',
                      script)
        self.assertIn("rustfs_start", script)


class AptInstall(unittest.TestCase):
    """Every job installs Ubuntu's packages through .github/actions/apt-install, which reaches mirrors
    outside ubuntu.com when its names don't resolve, as for hours on 2026-11-11 (TASK-360)."""

    def test_every_workflow_and_action_installs_through_it(self):
        github = ROOT / ".github"
        users = []
        for path in sorted([*github.glob("workflows/*.yml"), *github.glob("actions/*/action.yml")]):
            if path == APT_INSTALL:
                continue
            text = path.read_text()
            self.assertNotIn("apt-get", text, path)
            self.assertNotRegex(text, r"\bapt (install|update)\b", path)
            if "actions/apt-install" in text:
                users.append(path.relative_to(github).as_posix())
        self.assertEqual(users, ["actions/guest-host/action.yml", "workflows/ci.yml",
                                 "workflows/hunt.yml", "workflows/nightly.yml"])

    def install(self, resolves, sources=None, update_status=0):
        """Runs apt-install.sh for clang and lld on the image's mirror list, with the names in
        `resolves` alone resolving, `sources` for ubuntu.sources, and apt-get's update ending with
        `update_status`: its status and output, the mirror list it leaves, and what apt-get was
        asked, a line a call."""
        directory = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, directory)
        bin_ = directory / "bin"
        bin_.mkdir()
        mirrors, ubuntu_sources = directory / "apt-mirrors.txt", directory / "ubuntu.sources"
        mirrors.write_text(IMAGE_MIRRORS)
        ubuntu_sources.write_text(sources or f"Types: deb\nURIs: mirror+file:{mirrors}\n")
        (directory / "resolves").write_text("".join(f"{name}\n" for name in resolves))
        for name, script in {
            "sudo": '"$@"\n',
            "getent": 'grep -qx "$2" "$FAKE/resolves"\n',
            "apt-get": 'echo "$*" >> "$FAKE/apt-get"\n[ "$1" != update ] || exit "$UPDATE"\n',
        }.items():
            (bin_ / name).write_text("#!/bin/bash\n" + script)
            (bin_ / name).chmod(0o755)
        done = subprocess.run(
            ["/bin/bash", APT_INSTALL.parent / "apt-install.sh", "clang", "lld"],
            env={"PATH": f"{bin_}:/usr/bin:/bin", "FAKE": str(directory), "UPDATE": str(update_status),
                 "FXDAX_APT_MIRRORS": str(mirrors), "FXDAX_APT_SOURCES": str(ubuntu_sources)},
            capture_output=True, text=True)
        asked = directory / "apt-get"
        return (done.returncode, done.stdout, mirrors.read_text(),
                asked.read_text() if asked.exists() else "")

    def test_ubuntu_coms_mirrors_first_when_they_resolve(self):
        status, _, mirrors, asked = self.install(UBUNTU_COM + FALLBACKS)
        self.assertEqual(status, 0)
        self.assertEqual(sorted(mirrors.splitlines()), sorted([
            "http://azure.archive.ubuntu.com/ubuntu/\tpriority:1",
            "https://archive.ubuntu.com/ubuntu/\tpriority:2",
            "https://security.ubuntu.com/ubuntu/\tpriority:3",
            "https://mirrors.edge.kernel.org/ubuntu/\tpriority:10",
            "https://mirrors.mit.edu/ubuntu/\tpriority:11",
        ]))
        self.assertEqual(asked, "update -o APT::Update::Error-Mode=any\ninstall -y clang lld\n")

    def test_names_that_dont_resolve_are_dropped_so_apt_waits_on_none(self):
        """2026-11-11: no ubuntu.com name resolved on GitHub's runners for hours."""
        status, out, mirrors, asked = self.install(FALLBACKS)
        self.assertEqual(status, 0)
        self.assertEqual(sorted(mirrors.splitlines()), [
            "https://mirrors.edge.kernel.org/ubuntu/\tpriority:10",
            "https://mirrors.mit.edu/ubuntu/\tpriority:11",
        ])
        for name in UBUNTU_COM:
            self.assertIn(f"::warning::{name} doesn't resolve: apt won't try it\n", out)
        self.assertEqual(asked, "update -o APT::Update::Error-Mode=any\ninstall -y clang lld\n")

    def test_no_name_resolving_fails_before_apt(self):
        status, out, mirrors, asked = self.install([])
        self.assertEqual((status, mirrors, asked), (1, IMAGE_MIRRORS, ""))
        self.assertIn("::error::none of the Ubuntu mirrors' names resolve\n", out)

    def test_an_image_whose_apt_lists_no_mirror_file_fails(self):
        """Mirrors listed where apt never looks would protect nothing."""
        status, out, mirrors, asked = self.install(
            UBUNTU_COM, sources="URIs: http://archive.ubuntu.com/ubuntu/\n")
        self.assertEqual((status, mirrors, asked), (1, IMAGE_MIRRORS, ""))
        self.assertIn("::error::the runner's apt doesn't fetch through", out)

    def test_an_update_failing_fails_the_install(self):
        """apt's update's failure, an index no mirror served, is the action's."""
        status, _, _, asked = self.install(UBUNTU_COM + FALLBACKS, update_status=100)
        self.assertEqual((status, asked), (100, "update -o APT::Update::Error-Mode=any\n"))
