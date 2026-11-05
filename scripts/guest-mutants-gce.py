#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""The guest's hand mutants that need a real kernel, run on a GCE test host (TASK-44.19,
TASK-160.32).

Every past bug put back is a patch of guest/mutants, whose header names its killer and the kernel
it needs (guest/mutants/README.md). guest/sim/mutants.sh runs those of the simulator's, in
seconds; these are the rest, each a guest kernel's (release, debug, or kasan), its killer a guest
case, and the check of it that fails if the case is a checks suite:

    scripts/guest-mutants-gce.py HOST ZONE RESULTS [MUTANT...]
    scripts/guest-mutants-gce.py --check

runs each (or those named) on HOST, made with scripts/test-gce.sh create: in a worktree of HEAD
with its patch in, scripts/test-gce.sh runs its killer's case alone, of its checks the killer's
alone (FXDAX_CHECKS), FXDAX_E2E_ROUNDS times, on a guest kernel of its kind built from the
worktree; the host keeps its kernel's build directory, so each mutant recompiles fs/fxfs alone. It
writes each run's results under RESULTS, prints each mutant's fate, and exits 1 unless each was
killed. A mutant is killed only if its killer failed, as the run's summary says
(scripts/guest_results.py); MISSED if not; BROKEN if the run failed otherwise, as a build that
doesn't compile or a host lost, which says nothing of the killer. One whose killer is `none` isn't
run. FXDAX_MUTANT_KERNEL is the kernel of every mutant, in place of each's own; the other FXDAX_*
settings reach the host as scripts/test-gce.sh says.

--check runs nothing: every mutant's header names a kernel and a killer there is, and each of these
patches applies to the sources as they are, as CI checks; guest/sim/mutants.sh --check applies the
simulator's, which take a kernel's tree.
"""

import importlib.util
import os
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location("guest_results", ROOT / "scripts" / "guest_results.py")
guest_results = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(guest_results)
MUTANTS = ROOT / "guest" / "mutants"
SIM = ROOT / "guest" / "sim"
CASES = ROOT / "guest" / "tests" / "test_cases" / "src" / "lib.rs"
CHECKS = ROOT / "guest" / "tests" / "test_cases" / "src" / "test_fxfs_guest_checks" / "mod.rs"
# The kernels a mutant may need: the simulator, or a guest kernel scripts/test-gce.sh's host
# builds (FXDAX_GUEST_KERNEL_BUILD).
KERNELS = {"sim", "release", "debug", "kasan"}


@dataclass(frozen=True)
class Mutant:
    """A past bug put back: guest/mutants/NAME.patch, and what its header says of it."""

    name: str
    path: Path
    # Where it runs: "sim", or the guest kernel it needs.
    kernel: str
    # Its killer's words: a test of the simulator's and its arguments, or a guest case and the
    # check of it that fails; () if nothing kills it, its header saying why.
    killer: tuple

    @property
    def real(self):
        """Whether it needs a real kernel, which this runs it on."""
        return self.kernel != "sim"


def mutant(path):
    """The mutant of the patch `path`, as its header's `# Kernel:` and `# Killer:` say."""
    fields = dict(re.findall(r"^# (Kernel|Killer): (.*)$", path.read_text(), re.MULTILINE))
    killer = tuple(fields.get("Killer", "").split())
    return Mutant(path.stem, path, fields.get("Kernel", ""), () if killer == ("none",) else killer)


def mutants():
    """Every mutant of guest/mutants, by name."""
    return [mutant(path) for path in sorted(MUTANTS.glob("*.patch"))]


def runs():
    """What a header may name as a killer: the guest's cases, its checks suites' checks, the
    simulator's tests, and the scenarios of its tests/scenarios."""
    scenarios = (SIM / "tests" / "scenarios.c").read_text()
    return {
        "cases": set(re.findall(r'TestCase::new\(\s*"([a-z0-9-]+)"', CASES.read_text())),
        "checks": set(re.findall(r"^ +(\w+): \w+, \d+ =>", CHECKS.read_text(), re.MULTILINE)),
        "sim": {path.stem for path in (SIM / "tests").glob("*.c")},
        "scenarios": set(re.findall(r"^\t(?:COW_)?SCENARIO\((\w+)\),$", scenarios, re.MULTILINE)),
    }


def unsound(each, known):
    """What is wrong with the header of `each`, a mutant, given the runs there are (`runs()`),
    or None."""
    if each.kernel not in KERNELS:
        return f"its kernel is {each.kernel!r}, none of {sorted(KERNELS)}"
    if not each.killer:
        has = re.search(r"^# Killer: none$", each.path.read_text(), re.MULTILINE)
        return None if has else "its header names no killer"
    test, *arguments = each.killer
    if not each.real:
        if test not in known["sim"]:
            return f"its killer {test!r} is no test of guest/sim's"
        if test == "scenarios" and (len(arguments) != 1 or arguments[0] not in known["scenarios"]):
            return f"its killer {' '.join(each.killer)!r} is no scenario of tests/scenarios"
        return None
    if test not in known["cases"]:
        return f"its killer {test!r} is no guest case"
    if len(arguments) > 1 or arguments and arguments[0] not in known["checks"]:
        return f"its killer {' '.join(each.killer)!r} names no check of the guest's checks suites"
    return None


def check():
    """What is wrong with the mutants, a line for each wrong: a header naming no kernel or killer
    there is, or a real kernel's patch that doesn't apply to the sources as they are."""
    known, broken = runs(), []
    for each in mutants():
        wrong = unsound(each, known)
        if wrong is None and each.real:
            applied = subprocess.run(["git", "-C", ROOT, "apply", "--check", each.path],
                                     capture_output=True, text=True)
            if applied.returncode != 0:
                wrong = f"it doesn't apply: {applied.stderr.strip()}"
        if wrong:
            broken.append(f"{each.name}: {wrong}")
    return broken


def fate(status, summary, killer):
    """A mutant's fate, and what its run said, from the run's exit status and its guest tests'
    summary (scripts/guest_results.py), None if they never started, given its killer, a case and
    the check of it to fail, if any: killed only if the killer failed, the case failing a round, or
    running out of time in one, with its check among those failed (TASK-160.31)."""
    case, *check = killer
    ran = next((each for each in summary["cases"] if each["case"] == case), None) if summary else None
    failed = ran is not None and ran["outcome"] in guest_results.FAILED
    if ran is None or (status != 0 and not failed):
        return "BROKEN", f"the run ended with {status}, {case} {ran['outcome'] if ran else 'never ran'}"
    if not failed:
        return "MISSED", f"{case} passed"
    said = ", ".join(ran["failed_checks"]) or ran["reason"]
    if check and check[0] not in ran["failed_checks"]:
        return "MISSED", f"{case} failed, but not {check[0]}: {said}"
    return "killed", f"{case} {ran['outcome']}: {said}"


def run(host, zone, results, each, kernel):
    """Runs the mutant `each` on `host`: its fate, and what its run said."""
    case, *check = each.killer
    with tempfile.TemporaryDirectory(prefix="fxdax-mutant-") as scratch:
        tree = Path(scratch) / "tree"
        subprocess.run(["git", "-C", ROOT, "worktree", "add", "-q", "--detach", tree, "HEAD"],
                       check=True)
        try:
            subprocess.run(["git", "-C", tree, "submodule", "update", "--init", "-q",
                            "vendor/firecracker"], check=True)
            subprocess.run(["git", "-C", tree, "apply", each.path], check=True)
            env = dict(os.environ, FXDAX_GCE_ZONE=zone, FXDAX_WORKSPACE_TESTS="0",
                       FXDAX_GUEST_KERNEL_BUILD=kernel, FXDAX_E2E_TEST=case)
            env.pop("FXDAX_CHECKS", None)
            env.update({"FXDAX_CHECKS": check[0]} if check else {})
            out = Path(results) / each.name
            with open(Path(results) / f"{each.name}.log", "w") as log:
                ran = subprocess.run([tree / "scripts/test-gce.sh", "run", host, out], env=env,
                                     stdout=log, stderr=subprocess.STDOUT, cwd=tree)
            return fate(ran.returncode, guest_results.load(out / "e2e"), each.killer)
        finally:
            subprocess.run(["git", "-C", ROOT, "worktree", "remove", "--force", tree], check=True)


def main():
    if sys.argv[1:] == ["--check"]:
        broken = check()
        for line in broken:
            print(line, file=sys.stderr)
        print(f"mutants: {len(mutants())} checked, {len(broken)} broken")
        raise SystemExit(1 if broken else 0)
    if len(sys.argv) < 4:
        raise SystemExit(__doc__)
    host, zone, results, names = sys.argv[1], sys.argv[2], sys.argv[3], set(sys.argv[4:])
    chosen = [each for each in mutants() if each.real and (not names or each.name in names)]
    unknown = names - {each.name for each in chosen}
    if unknown:
        raise SystemExit(f"no mutant of a real kernel is named {', '.join(sorted(unknown))}")
    Path(results).mkdir(parents=True, exist_ok=True)
    survived = 0
    for each in chosen:
        if not each.killer:
            print(f"none {each.name}: nothing kills it: see its header", flush=True)
            continue
        kernel = os.environ.get("FXDAX_MUTANT_KERNEL") or each.kernel
        ended, said = run(host, zone, results, each, kernel)
        survived += ended != "killed"
        print(f"{ended} {each.name} ({' '.join(each.killer)}, {kernel}): {said}", flush=True)
    raise SystemExit(1 if survived else 0)


if __name__ == "__main__":
    main()
