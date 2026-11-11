# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of .github/workflows/ci.yml's test job: each part installs what its tests run, and no part
what only another's does. Every part fetching Coccinelle and its OCaml, three quarters of the
downloads, a slow mirror took 40 minutes of the workspace part's 90, which its tests then outran
(run 42089880804, TASK-172)."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
CI = ROOT / ".github" / "workflows" / "ci.yml"
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
    install = re.search(r"sudo apt-get install -y ((?:.*\\\n)*.*)$", job(), re.MULTILINE)
    common = set(install.group(1).replace("\\", " ").split()) - {"${{", "matrix.packages", "}}"}
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
