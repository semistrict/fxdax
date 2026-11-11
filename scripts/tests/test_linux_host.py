# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/lib/linux-host-setup.sh, the library of the scripts that run on a host, here
with a temporary directory as the host's work directory; and of the tree the Lima VM's scripts
run in, which scripts/lib/lima.sh syncs with it, here with a temporary directory as the VM's home,
where a limactl in its place runs the VM's shell."""

import contextlib
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

from script import SCRIPTS

LIBRARY = SCRIPTS / "lib" / "linux-host-setup.sh"
# When a run's checkout was made, and when an earlier run built the host's tree: later.
CHECKED_OUT = 978307200
BUILT = 1420070400
# What the tree built, which stays, and what the checkout's did, which is not copied.
BUILDS = ["target/debug/x", "guest/tests/target/debug/x", "vendor/firecracker/build/x"]
# The VM's shell: `limactl shell INSTANCE -- bash -lc SCRIPT` runs SCRIPT here, its home $FAKE_VM,
# in an environment of its own, as Lima's.
LIMACTL = """#!/bin/bash
[[ $1 == shell && $3 == -- && $4 == bash ]] || { echo "limactl: no such call: $*" >&2; exit 2; }
shift 4
exec env -i HOME="$FAKE_VM" PATH=/usr/bin:/bin bash "$@"
"""
# A script the VM runs, which says what it was given.
SAYS = '#!/bin/bash\necho "$0 $*, LIBCLANG_PATH=${LIBCLANG_PATH:-}"\n'


def write(path, text, mtime):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    os.utime(path, (mtime, mtime))


def files(directory):
    return sorted(str(path.relative_to(directory)) for path in directory.rglob("*") if path.is_file())


def built(tree, source):
    """`tree`, as an earlier run built it, and `source`, a checkout made before that run: a file
    changed since, of the same size, so that only its content tells; one unchanged; one removed;
    what each built; Git's; and a directory named build that is source."""
    write(source / "src" / "changed.rs", "new\n", CHECKED_OUT)
    write(tree / "src" / "changed.rs", "old\n", BUILT)
    write(source / "src" / "same.rs", "same\n", CHECKED_OUT)
    write(tree / "src" / "same.rs", "same\n", BUILT)
    write(tree / "src" / "gone.rs", "gone\n", BUILT)
    for build in BUILDS:
        write(tree / build, "built here\n", BUILT)
        write(source / build, "built there\n", CHECKED_OUT)
    write(source / ".git" / "HEAD", "ref: refs/heads/main\n", CHECKED_OUT)
    write(source / "crates" / "c" / "build" / "build.rs", "fn main() {}\n", CHECKED_OUT)


class Synced:
    def assertSynced(self, tree, start):
        """That `tree`, as built() has it, now has the checkout's content, written where it
        differed, and its own builds; the scripts aside, which the checkout may have."""
        self.assertEqual(
            [file for file in files(tree) if not file.startswith("scripts/")],
            [
                ".by-content",
                "crates/c/build/build.rs",
                "guest/tests/target/debug/x",
                "src/changed.rs",
                "src/same.rs",
                "target/debug/x",
                "vendor/firecracker/build/x",
            ],
        )
        self.assertEqual((tree / "src" / "changed.rs").read_text(), "new\n")
        # Newer than the build, which cargo then rebuilds.
        self.assertGreaterEqual(int((tree / "src" / "changed.rs").stat().st_mtime), start)
        self.assertEqual((tree / "src" / "same.rs").stat().st_mtime, BUILT)
        self.assertEqual([(tree / build).read_text() for build in BUILDS], ["built here\n"] * 3)


class Tree(Synced, unittest.TestCase):
    def host_tree(self, work, source):
        result = subprocess.run(
            [
                "/bin/bash",
                "-c",
                'set -euo pipefail; source "$1"; work=$2 repo=$3; host_tree t; printf %s "$repo"',
                "bash",
                LIBRARY,
                work,
                source,
            ],
            # Not the environment of a host run's, whose trees build in place (InPlace).
            env={"PATH": os.environ["PATH"]},
            capture_output=True,
            text=True,
        )
        self.assertEqual((result.returncode, result.stderr, result.stdout), (0, "", str(work / "t")))

    def test_the_tree_is_written_where_it_differs_and_keeps_its_builds(self):
        work, source = Path(tempfile.mkdtemp()), Path(tempfile.mkdtemp())
        built(work / "t", source)
        # Synced by content before.
        (work / "t" / ".by-content").touch()
        start = int(time.time())
        self.host_tree(work, source)
        self.assertSynced(work / "t", start)

    def test_a_tree_synced_by_times_is_written_whole_once(self):
        # Synced by times, as trees were (rsync -a): its content may be newer than its times say,
        # beside a build made from what was there before.
        work, source = Path(tempfile.mkdtemp()), Path(tempfile.mkdtemp())
        built(work / "t", source)
        start = int(time.time())
        self.host_tree(work, source)
        self.assertGreaterEqual(int((work / "t" / "src" / "same.rs").stat().st_mtime), start)
        os.utime(work / "t" / "src" / "same.rs", (BUILT, BUILT))
        self.host_tree(work, source)
        self.assertEqual((work / "t" / "src" / "same.rs").stat().st_mtime, BUILT)

    def test_a_tree_copied_by_times_since_is_written_whole_again(self):
        # Copied by content, then by times, as the scripts before did (rsync -a --delete): the
        # changed file has the checkout's time again, older than the tree's build, and the content
        # the copy by content wrote.
        work, source = Path(tempfile.mkdtemp()), Path(tempfile.mkdtemp())
        built(work / "t", source)
        self.host_tree(work, source)
        subprocess.run(
            ["rsync", "-a", "--delete", "--exclude", "target", "--exclude", ".git",
             f"{source}/", f"{work / 't'}/"],
            check=True,
        )
        self.assertEqual((work / "t" / "src" / "changed.rs").stat().st_mtime, CHECKED_OUT)
        start = int(time.time())
        self.host_tree(work, source)
        self.assertGreaterEqual(int((work / "t" / "src" / "changed.rs").stat().st_mtime), start)


class InPlace(unittest.TestCase):
    def test_a_runs_own_tree_builds_where_it_is(self):
        """A host run's worktree (scripts/lib/host.sh, FXDAX_HOST_IN_PLACE=1) is its own, its
        target directories too: host_tree copies nothing, and repo stays, its .git with it."""
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source"
            write(source / ".git", "gitdir: elsewhere\n", CHECKED_OUT)
            done = subprocess.run(
                ["/bin/bash", "-c",
                 'set -euo pipefail; source "$0"; work=$1 repo=$2; host_tree t; printf %s "$repo"',
                 LIBRARY, Path(directory) / "work", source],
                env={"PATH": "/usr/bin:/bin", "FXDAX_HOST_IN_PLACE": "1"},
                capture_output=True, text=True)
            self.assertEqual((done.returncode, done.stdout, done.stderr), (0, str(source), ""))
            self.assertFalse((Path(directory) / "work").exists())


class Lima(Synced, unittest.TestCase):
    """The Lima VM's tree, ~/fxdax-test, which scripts/lib/lima.sh syncs before it runs a script
    there, from a checkout of its own."""

    def lima(self, script, *args):
        """Runs the checkout's scripts/`script` with `args`, the checkout and the VM's tree as
        built() has them, the tree synced by content before: its status, output and errors."""
        self.vm, checkout = Path(tempfile.mkdtemp()), Path(tempfile.mkdtemp())
        built(self.vm / "fxdax-test", checkout)
        (self.vm / "fxdax-test" / ".by-content").touch()
        for name in ["lib/lima.sh", "lib/linux-host-setup.sh", "test-linux.sh"]:
            (checkout / "scripts" / name).parent.mkdir(parents=True, exist_ok=True)
            (checkout / "scripts" / name).symlink_to(SCRIPTS / name)
        for name in ["bench.sh", "test.sh"]:
            (checkout / "scripts" / name).write_text(SAYS)
            (checkout / "scripts" / name).chmod(0o755)
        bin_ = Path(tempfile.mkdtemp())
        (bin_ / "limactl").write_text(LIMACTL)
        (bin_ / "limactl").chmod(0o755)
        self.start = int(time.time())
        done = subprocess.run(
            ["/bin/bash", checkout / "scripts" / script, *args],
            env={
                "PATH": f"{bin_}:/usr/bin:/bin",
                "HOME": tempfile.mkdtemp(),
                "FAKE_VM": str(self.vm),
                "LIBCLANG_PATH": "/usr/lib/llvm 19/lib",
            },
            capture_output=True,
            text=True,
        )
        return done.returncode, done.stdout, done.stderr

    def test_a_script_runs_in_the_vms_tree_synced_by_content(self):
        ran = self.lima("lib/lima.sh", "bench.sh", "3", "a b")
        script = self.vm / "fxdax-test" / "scripts" / "bench.sh"
        self.assertEqual(ran, (0, f"{script} 3 a b, LIBCLANG_PATH=/usr/lib/llvm 19/lib\n", ""))
        self.assertSynced(self.vm / "fxdax-test", self.start)

    def test_the_tests_run_in_the_vms_tree_synced_by_content(self):
        ran = self.lima("test-linux.sh", "-E", "test(x)")
        script = self.vm / "fxdax-test" / "scripts" / "test.sh"
        self.assertEqual(ran, (0, f"{script} -E test(x), LIBCLANG_PATH=/usr/lib/llvm 19/lib\n", ""))
        self.assertSynced(self.vm / "fxdax-test", self.start)


class Apt(unittest.TestCase):
    """host_apt, what installs a host's packages, restarting no service under the run's job:
    needrestart restarted sshd and re-executed systemd after a run's packages went in, and the
    job died with no status 11 s later (GCE r990)."""

    def test_apt_get_runs_restarting_no_service(self):
        with tempfile.TemporaryDirectory() as directory:
            tools = Path(directory)
            apt = tools / "apt-get"
            apt.write_text(
                "#!/bin/bash\n"
                'echo "NEEDRESTART_SUSPEND=${NEEDRESTART_SUSPEND:-}'
                ' DEBIAN_FRONTEND=${DEBIAN_FRONTEND:-} $*"\n'
            )
            apt.chmod(0o755)
            done = subprocess.run(
                ["/bin/bash", "-c", 'source "$0"; host_apt install -y -qq podman', LIBRARY],
                env={"PATH": f"{tools}:/usr/bin:/bin"},
                capture_output=True,
                text=True,
            )
        said = "NEEDRESTART_SUSPEND=1 DEBIAN_FRONTEND=noninteractive"
        said += " -o DPkg::Lock::Timeout=600 install -y -qq podman\n"
        self.assertEqual((done.returncode, done.stdout, done.stderr), (0, said, ""))

    def test_the_hosts_scripts_install_through_host_apt(self):
        # Every apt-get a run's host script calls, but host_apt's own.
        calls = [
            f"{path.name}:{number}"
            for path in sorted((SCRIPTS / "lib").glob("*.sh"))
            for number, line in enumerate(path.read_text().splitlines(), 1)
            if "apt-get" in line
            and not line.lstrip().startswith("#")
            and "DPkg::Lock::Timeout=600 \"$@\"" not in line
        ]
        self.assertEqual(calls, [])

    def test_the_hosts_that_run_the_guest_tests_install_nextest(self):
        # scripts/test-e2e.sh, which bench.sh runs its cases through, runs scripts/test.sh's
        # firecracker part, under nextest (TASK-220): every host script reaching it needs it, or the
        # run fails there (GCE r1022, the bench host's). But for one whose runs build and run
        # artifacts built once (FXDAX_E2E_BUILD_ONLY, FXDAX_E2E_PREBUILT), which run no tests of
        # the VMM's (bench-ab-linux-host.sh, TASK-276).
        reaching = ("scripts/test-e2e.sh", "scripts/bench.sh")
        missing = [
            path.name
            for path in sorted((SCRIPTS / "lib").glob("*-linux-host.sh"))
            for text in [path.read_text()]
            if any(script in text for script in reaching)
            and "host_need cargo-nextest" not in text
            and "FXDAX_E2E_PREBUILT" not in text
        ]
        self.assertEqual(missing, [])


class Network(unittest.TestCase):
    """host_network: a fetch that stalls fails, rather than hang the run: GCE r940 waited 2 h
    40 min at cargo's "Updating crates.io index", its connection dropped."""

    def test_a_stalled_fetch_fails(self):
        # A registry that takes each connection and says nothing on it, holding it.
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen(16)
        held = []

        def hold():
            # Each connection, until the listener closes as the test ends.
            with contextlib.suppress(OSError):
                while True:
                    held.append(listener.accept()[0])

        threading.Thread(target=hold, daemon=True).start()
        port = listener.getsockname()[1]
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory)
            (project / "src").mkdir()
            (project / "src" / "lib.rs").write_text("")
            (project / "Cargo.toml").write_text(
                '[package]\nname = "stalled"\nversion = "0.1.0"\nedition = "2021"\n'
                '[dependencies]\nnone-such = "1"\n'
            )
            (project / ".cargo").mkdir()
            (project / ".cargo" / "config.toml").write_text(
                '[source.crates-io]\nreplace-with = "silent"\n'
                f'[source.silent]\nregistry = "sparse+http://127.0.0.1:{port}/"\n'
            )
            # rustup's choice of toolchain too, where a host makes it by its environment, with no
            # default (TASK-279).
            rustup = {
                var: os.environ[var] for var in ("RUSTUP_HOME", "RUSTUP_TOOLCHAIN") if var in os.environ
            }
            environment = rustup | {
                "PATH": os.environ["PATH"],
                "HOME": os.environ["HOME"],
                "CARGO_HOME": str(project / "cargo-home"),
                # Its bounds scaled down: a minute, five tries, as seconds, one.
                "CARGO_HTTP_TIMEOUT": "2",
                "CARGO_NET_RETRY": "1",
            }
            started = time.monotonic()
            done = subprocess.run(
                ["/bin/bash", "-c", 'source "$0"; host_network; cargo generate-lockfile', LIBRARY],
                cwd=project,
                env=environment,
                capture_output=True,
                text=True,
                # A hang, the bug, ends here.
                timeout=120,
            )
            took = time.monotonic() - started
        listener.close()
        for connection in held:
            connection.close()
        self.assertEqual(done.returncode, 101, done.stderr)
        # Its low-speed limit host_network's, a KiB a second.
        stalled = "transfer too slow: failed to transfer more than 1024 bytes in 2s"
        self.assertIn(f"{stalled} (transferred 0 bytes)", done.stderr)
        self.assertLess(took, 60)

    def test_fetches_are_bounded_as_the_caller_leaves_them(self):
        script = 'source "$0"; host_network; env | grep ^CARGO_ | sort'
        done = subprocess.run(
            ["/bin/bash", "-c", script, LIBRARY],
            env={"PATH": "/usr/bin:/bin", "CARGO_NET_RETRY": "2"},
            capture_output=True,
            text=True,
        )
        said = [
            "CARGO_HTTP_LOW_SPEED_LIMIT=1024",
            "CARGO_HTTP_MULTIPLEXING=false",
            "CARGO_HTTP_TIMEOUT=60",
            "CARGO_NET_RETRY=2",
        ]
        self.assertEqual((done.returncode, done.stdout.splitlines(), done.stderr), (0, said, ""))

class Provisioned(unittest.TestCase):
    """A host of one's own, provisioned already (ubu1): FXDAX_HOST_WORK names the work directory,
    and with FXDAX_HOST_PROVISIONED=1 host_need takes a tool there at the version it is, where it
    would install the pinned one (TASK-276.6). Here cargo is a fake whose nextest says a version
    other than the pin, and which fails anything else, an install among them."""

    CARGO = """#!/bin/bash
[[ $1 == nextest && $2 == --version ]] || { echo "cargo: $*" >&2; exit 3; }
echo "cargo-nextest 0.0.1"
"""

    def run_library(self, script, environment):
        with tempfile.TemporaryDirectory() as directory:
            cargo = Path(directory) / "cargo"
            cargo.write_text(self.CARGO)
            cargo.chmod(0o755)
            environment = {"PATH": f"{directory}:/usr/bin:/bin", **environment}
            return subprocess.run(
                ["/bin/bash", "-c", f'set -euo pipefail; source "$0"; {script}', LIBRARY],
                env=environment,
                capture_output=True,
                text=True,
            )

    def test_the_work_directory_is_the_callers(self):
        done = self.run_library('printf %s "$work"', {"FXDAX_HOST_WORK": "/scratch/work"})
        self.assertEqual((done.returncode, done.stdout, done.stderr), (0, "/scratch/work", ""))
        done = self.run_library('printf %s "$work"', {})
        self.assertEqual((done.returncode, done.stdout), (0, "/var/lib/fxdax-test"))

    def test_a_provisioned_hosts_tools_are_taken_as_they_are(self):
        script = 'host_need cargo-nextest; printf "%s\\n" "${host_needed[@]}"'
        done = self.run_library(script, {"FXDAX_HOST_PROVISIONED": "1"})
        self.assertEqual((done.returncode, done.stdout, done.stderr), (0, "cargo-nextest\n", ""))

    def test_another_hosts_tools_are_installed_at_their_pins(self):
        done = self.run_library("host_need cargo-nextest", {})
        self.assertEqual(done.returncode, 3)
        self.assertIn("cargo: install cargo-nextest --version 0.9.143 --locked", done.stderr)


class Rust(unittest.TestCase):
    """host_rust: the toolchain the hosts build with, installed unless there and working. A job
    killed as rustup installed it, with its SSH session, left a toolchain with no manifest, and
    every run on the host after failed at its first rustup or cargo, "missing manifest in toolchain"
    (GCE r980). Here rustup and rustc are fakes that keep the toolchain as rustup's directories do:
    rustc runs only if the toolchain's manifest is there, and rustup's install puts it there."""

    TOOLCHAIN = "1.97.1"
    FULL = "1.97.1-x86_64-unknown-linux-gnu"
    MANIFEST = "lib/rustlib/multirust-channel-manifest.toml"
    # Each records its call; rustc, and rustup's other commands, fail without the manifest.
    RUSTC = """#!/bin/bash
toolchain="$RUSTUP_HOME/toolchains/$RUSTUP_TOOLCHAIN-x86_64-unknown-linux-gnu"
[[ -f $toolchain/lib/rustlib/multirust-channel-manifest.toml ]] ||
    { echo "error: missing manifest in toolchain '$RUSTUP_TOOLCHAIN'" >&2; exit 1; }
echo "rustc $RUSTUP_TOOLCHAIN"
"""
    RUSTUP = """#!/bin/bash
echo "rustup $*" >> "$CALLS"
toolchain="$RUSTUP_HOME/toolchains/$RUSTUP_TOOLCHAIN-x86_64-unknown-linux-gnu"
if [[ $1 == toolchain && $2 == install ]]; then
    mkdir -p "$toolchain/lib/rustlib" "$RUSTUP_HOME/update-hashes"
    touch "$toolchain/lib/rustlib/multirust-channel-manifest.toml"
    echo hash > "$RUSTUP_HOME/update-hashes/$RUSTUP_TOOLCHAIN-x86_64-unknown-linux-gnu"
    exit 0
fi
[[ -f $toolchain/lib/rustlib/multirust-channel-manifest.toml ]] ||
    { echo "error: missing manifest in toolchain '$RUSTUP_TOOLCHAIN'" >&2; exit 1; }
"""

    def host_rust(self, home, toolchain_files):
        """host_rust's status and errors, and the rustup calls it made, on a host whose rustup is
        installed, in `home`, and whose toolchain holds `toolchain_files`, the update hash naming
        it."""
        cargo, rustup = home / "cargo", home / "rustup"
        tools = cargo / "bin"
        tools.mkdir(parents=True)
        for name, text in (("rustc", self.RUSTC), ("rustup", self.RUSTUP), ("cargo", "")):
            (tools / name).write_text(text)
            (tools / name).chmod(0o755)
        for name in toolchain_files:
            path = rustup / "toolchains" / self.FULL / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("")
        (rustup / "update-hashes").mkdir(parents=True)
        (rustup / "update-hashes" / self.FULL).write_text("hash\n")
        calls = home / "calls"
        calls.write_text("")
        done = subprocess.run(
            ["/bin/bash", "-c", 'source "$0"; host_rust', LIBRARY],
            env={
                "PATH": f"{tools}:/usr/bin:/bin",
                "CARGO_HOME": str(cargo),
                "RUSTUP_HOME": str(rustup),
                "RUSTUP_TOOLCHAIN": self.TOOLCHAIN,
                "CALLS": str(calls),
            },
            capture_output=True,
            text=True,
        )
        return done.returncode, done.stderr, calls.read_text().splitlines()

    def test_a_toolchain_installed_part_way_is_installed_again(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            # Its binaries, some; no manifest, which rustup writes last.
            done = self.host_rust(home, ["bin/rustc", "lib/librustc_driver.so"])
            toolchain = home / "rustup" / "toolchains" / self.FULL
            left = sorted(str(path.relative_to(toolchain)) for path in toolchain.rglob("*"))
        self.assertEqual(
            done,
            (
                0,
                "",
                [
                    f"rustup toolchain install {self.TOOLCHAIN} --profile minimal",
                    "rustup target add x86_64-unknown-linux-musl",
                    "rustup component add clippy",
                ],
            ),
        )
        # What the install left part way gone, the toolchain installed anew.
        self.assertEqual(left, ["lib", "lib/rustlib", self.MANIFEST])

    def test_a_working_toolchain_is_kept(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            done = self.host_rust(home, ["bin/rustc", self.MANIFEST])
        calls = ["rustup target add x86_64-unknown-linux-musl", "rustup component add clippy"]
        self.assertEqual(done, (0, "", calls))


class Kernels(unittest.TestCase):
    """host_kernel, a run's guest kernel: built in the run's work directory, or with a cache of
    kernels on the host (FXDAX_HOST_KERNELS), taken from there if one was built from the same
    sources. Here a source tree of the kernel's inputs and one that isn't, whose
    build-guest-kernel.sh is a fake that says what it built and where, its kernel naming the
    sources' text; cargo a fake whose metadata, run in the tree's root (scripts/guest-kernel-key.sh),
    names guest/rust's packages of the workspace's own;
    and flock fake_flock.py."""

    BUILD = """#!/bin/bash
echo "built $(basename "$FXDAX_GUEST_KERNEL_WORK") debug=$FXDAX_GUEST_KERNEL_DEBUG \\
coverage=$FXDAX_GUEST_KERNEL_COVERAGE" >> "$CALLS"
mkdir -p "$FXDAX_GUEST_KERNEL_WORK/out/x86_64"
cat "$(dirname "$0")/../guest/linux/fs.c" "$(dirname "$0")/../crates/fxfs/lib.rs" \\
    > "$FXDAX_GUEST_KERNEL_WORK/out/x86_64/vmlinux"
"""
    CARGO = """#!/bin/bash
[[ $1 == metadata ]] || { echo "cargo: $*" >&2; exit 3; }
echo '{"packages": [{"source": null, "manifest_path": "'"$PWD"'/guest/rust/fxfs-linux/Cargo.toml"},
    {"source": null, "manifest_path": "'"$PWD"'/guest/rust/../../crates/fxfs/Cargo.toml"},
    {"source": "registry+https://github.com/rust-lang/crates.io-index",
     "manifest_path": "/registry/serde/Cargo.toml"}]}'
"""

    def setUp(self):
        self.directory = Path(tempfile.mkdtemp())
        self.repo = self.directory / "repo"
        for name, text in {
            "Cargo.toml": "[workspace]\n",
            "guest/linux/fs.c": "fs\n",
            "guest/rust/rust-toolchain.toml": '[toolchain]\nchannel = "nightly-2026-10-12"\n',
            "guest/rust/fxfs-linux/Cargo.toml": "[package]\n",
            "crates/fxfs/Cargo.toml": "[package]\n",
            "crates/fxfs/lib.rs": "fxfs\n",
            # The VMM's, which the kernel isn't built of.
            "crates/fxdax/lib.rs": "vmm\n",
            "scripts/build-guest-kernel.sh": self.BUILD,
            "scripts/build-guest-sysroot.sh": "",
        }.items():
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
            path.chmod(0o755)
        tools = self.directory / "bin"
        tools.mkdir()
        for name, text in {
            "cargo": self.CARGO,
            "rustup": "#!/bin/sh\n",
            "flock": f'#!/bin/sh\nexec {sys.executable} {SCRIPTS / "tests" / "fake_flock.py"} "$@"\n',
        }.items():
            (tools / name).write_text(text)
            (tools / name).chmod(0o755)
        self.calls = self.directory / "calls"
        self.calls.write_text("")
        self.cache = self.directory / "kernels"

    def kernel(self, kind, work, cached=True):
        """host_kernel KIND's builds, a run's work directory `work`, with the cache if `cached`:
        its status and errors, what it built, and its kernel's path and text."""
        built_before = self.calls.read_text()
        done = subprocess.run(
            ["/bin/bash", "-c", 'set -euo pipefail; source "$0"; repo=$2 results=$PWD/results; '
             'mkdir -p "$results"; host_kernel "$1"; printf %s "$host_kernel_image"', LIBRARY, kind,
             self.repo],
            env={"PATH": f"{self.directory / 'bin'}:/usr/bin:/bin:/sbin", "CALLS": str(self.calls),
                 "FXDAX_HOST_WORK": str(self.directory / work),
                 **({"FXDAX_HOST_KERNELS": str(self.cache)} if cached else {})},
            capture_output=True, text=True, cwd=self.directory,
        )
        built = self.calls.read_text()[len(built_before):].splitlines()
        image = Path(done.stdout) if done.stdout else None
        return done.returncode, done.stderr, built, image, image.read_text() if image else None

    def test_without_a_cache_a_kernel_builds_in_the_runs_work_directory(self):
        status, errors, built, image, text = self.kernel("debug", "w1", cached=False)
        self.assertEqual((status, errors, built), (0, "", ["built guest-kernel-debug debug=1 coverage=0"]))
        self.assertEqual((image, text), (self.directory / "w1/guest-kernel-debug/out/x86_64/vmlinux",
                                         "fs\nfxfs\n"))

    def test_a_kernel_of_the_same_sources_is_built_once(self):
        first = self.kernel("release", "w1")
        self.assertEqual(first[:3], (0, "", ["built work debug=0 coverage=0"]))
        self.assertEqual(first[3:], (self.directory / "w1/guest-kernel-release/vmlinux", "fs\nfxfs\n"))
        # Another run's: the same sources, but for what the kernel isn't built of.
        (self.repo / "crates" / "fxdax" / "lib.rs").write_text("another vmm\n")
        second = self.kernel("release", "w2")
        self.assertEqual(second[:3], (0, "", []))
        self.assertEqual(second[3:], (self.directory / "w2/guest-kernel-release/vmlinux", "fs\nfxfs\n"))
        # A change to its Rust, of a package of the workspace's own: built again, in the cache's
        # work directory, as the last was.
        (self.repo / "crates" / "fxfs" / "lib.rs").write_text("fxfs changed\n")
        third = self.kernel("release", "w3")
        self.assertEqual(third[2:], (["built work debug=0 coverage=0"],
                                     self.directory / "w3/guest-kernel-release/vmlinux",
                                     "fs\nfxfs changed\n"))

    def test_each_kind_is_a_kernel_of_its_own(self):
        self.assertEqual(self.kernel("release", "w1")[2], ["built work debug=0 coverage=0"])
        self.assertEqual(self.kernel("debug", "w2")[2], ["built work debug=1 coverage=0"])
        self.assertEqual(sorted(path.name for path in self.cache.iterdir()),
                         ["debug", "debug.lock", "release", "release.lock"])

    def test_a_coverage_kernel_is_never_taken_from_the_cache(self):
        # Its counts are read against its own build's notes.
        for work in ["w1", "w2"]:
            self.assertEqual(self.kernel("coverage", work)[2:4], (
                ["built guest-kernel-coverage debug=0 coverage=1"],
                self.directory / work / "guest-kernel-coverage/out/x86_64/vmlinux"))

    def test_the_cache_keeps_the_last_four(self):
        for n in range(6):
            (self.repo / "guest" / "linux" / "fs.c").write_text(f"fs {n}\n")
            self.assertEqual(self.kernel("release", f"w{n}")[0], 0)
        self.assertEqual(len(list((self.cache / "release" / "built").iterdir())), 4)

    def test_no_such_kind(self):
        status, errors, built, _, _ = self.kernel("fast", "w1")
        self.assertEqual((status, built), (2, []))
        self.assertEqual(errors, "host_kernel: a kernel is release, debug, kasan, coverage or "
                                 "rust-checks\n")


if __name__ == "__main__":
    unittest.main()
