# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/build-guest-kernel.sh's checks of the Rust it links into the kernel: that no
member of the Rust's archive defines the kernel's string functions, and that the object built from
it does not either. The script runs as it is, in a tree of its own beside the repository's guest/,
with tools in place of those that build: git, which makes the series' commits and the patches;
make, which configures and builds the kernel; cargo, which builds the Rust; nm, which lists what
the archive's members and the object define, as the test says; ar, which records the members it
deletes; nproc; and scripts/build-guest-sysroot.sh."""

import subprocess
import tempfile
import unittest
from pathlib import Path

from script import SCRIPTS

FUNCTIONS = ["memcpy", "memmove", "memset", "memcmp", "bcmp", "strlen"]
TOOLS = {
    # A clone with fs/fxfs, the series' patches, and a commit's, a tree's or a blob's name.
    "git": """\
if [[ $1 == -C ]]; then shift 2; fi
case $1 in
    clone) mkdir -p "${@: -1}/fs/fxfs" ;;
    format-patch) while [[ $1 != -o ]]; do shift; done; mkdir -p "$2" ;;
    rev-parse | hash-object | write-tree) echo 0123456789abcdef0123456789abcdef01234567 ;;
    commit-tree) cat > /dev/null; echo 0123456789abcdef0123456789abcdef01234567 ;;
esac
""",
    # The kernel's image, and the object of fs/fxfs's Rust.
    "make": """\
src=$2
if [[ ${@: -1} == vmlinux ]]; then
    : > "$src/vmlinux"
    : > "$src/fs/fxfs/fxfs_rust.o"
fi
""",
    "cargo": """\
mkdir -p "$CARGO_TARGET_DIR/x86_64-unknown-none/release"
: > "$CARGO_TARGET_DIR/x86_64-unknown-none/release/libfxfs_linux.a"
""",
    # What the archive's members define, by member, or what the object does, as the test says,
    # by one process, as nm's: a reader that stops early ends it.
    "nm": """\
if [[ $1 == -A ]]; then
    while read -r member symbol; do
        echo "$4:$member:0000000000000000 T $symbol"
    done < "$FAKE/archive"
    exit 0
fi
status=$(cat "$FAKE/status")
if (( status == 0 )); then exec cat "$FAKE/object"; fi
cat "$FAKE/object"
exit "$status"
""",
    "ar": 'echo "$3" >> "$FAKE/deleted"\n',
    "nproc": "echo 8\n",
}
MESSAGE = "fxfs_rust.o defines the string functions above, which must be the kernel's\n"


class RustInTheKernel(unittest.TestCase):
    def build(self, object_, archive="", status=0):
        """Builds the x86-64 kernel, nm listing `archive` (a member and a symbol it defines, a line
        each) for the Rust's archive, and `object_`, then ending with `status`, for its object:
        the script's status, output and errors."""
        self.directory = Path(tempfile.mkdtemp())
        fake, tree, bin_ = self.directory / "fake", self.directory / "tree", self.directory / "bin"
        for directory in [fake, tree / "scripts", bin_]:
            directory.mkdir(parents=True)
        for name, text in {"archive": archive, "object": object_, "status": str(status)}.items():
            (fake / name).write_text(text)
        (fake / "deleted").write_text("")
        (tree / "guest").symlink_to(SCRIPTS.parent / "guest")
        (tree / "scripts" / "build-guest-kernel.sh").symlink_to(SCRIPTS / "build-guest-kernel.sh")
        tools = {bin_ / name: script for name, script in TOOLS.items()}
        tools[tree / "scripts" / "build-guest-sysroot.sh"] = "echo -Cpanic=abort\n"
        for path, script in tools.items():
            path.write_text("#!/bin/bash\n" + script)
            path.chmod(0o755)
        self.work = self.directory / "work"
        done = subprocess.run(
            ["/bin/bash", tree / "scripts" / "build-guest-kernel.sh", "x86_64"],
            env={
                "PATH": f"{bin_}:/usr/bin:/bin",
                "HOME": str(self.directory),
                "FAKE": str(fake),
                "FXDAX_GUEST_KERNEL_WORK": str(self.work),
            },
            capture_output=True,
            text=True,
        )
        return done.returncode, done.stdout, done.stderr

    def built(self):
        return (self.work / "out" / "x86_64" / "vmlinux").exists()

    def test_a_kernel_whose_rust_uses_the_kernels_string_functions_is_built(self):
        self.assertEqual(
            self.build("                 U memcpy\n0000000000000000 T fxfs_init\n"),
            (0, f"{self.work / 'out' / 'x86_64' / 'vmlinux'}\n", ""),
        )
        self.assertTrue(self.built())

    def test_the_archive_loses_the_members_that_define_string_functions(self):
        archive = "".join(f"compiler_builtins-{i}.o {name}\n" for i, name in enumerate(FUNCTIONS))
        self.build("", archive + "fxfs_linux-0.o fxfs_init\ncompiler_builtins-6.o memcpy_fast\n")
        self.assertEqual(
            (self.directory / "fake" / "deleted").read_text().split(),
            [f"compiler_builtins-{i}.o" for i in range(len(FUNCTIONS))],
        )

    def test_rust_defining_a_string_function_fails_the_build(self):
        for name in FUNCTIONS:
            with self.subTest(name):
                definition = f"0000000000000000 T {name}\n"
                self.assertEqual(self.build(definition), (1, "", definition + MESSAGE))
                self.assertFalse(self.built())

    def test_a_definition_amid_much_else_fails_the_build(self):
        # More of nm's listing after it than a pipe holds.
        definition = "0000000000000000 W memset\n"
        status, _, errors = self.build(definition + "0000000000000000 T fxfs_other\n" * 100000)
        self.assertEqual((status, errors), (1, definition + MESSAGE))
        self.assertFalse(self.built())

    def test_nm_failing_fails_the_build(self):
        self.assertEqual(self.build("", status=1), (1, "", ""))
        self.assertFalse(self.built())


if __name__ == "__main__":
    unittest.main()
