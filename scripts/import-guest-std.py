#!/usr/bin/env python3
"""Imports parts of std into the guest kernel's std (guest/rust/std, TASK-26.6), from the rust-src
of the toolchain guest/rust/rust-toolchain.toml pins (Rust's library is MIT OR Apache-2.0).

Crates written for std need std's own types and methods, which a lookalike can't serve: they call
hash maps' methods by path (`HashMap::get(map, key)`) and name their entry types, and call the
float methods std adds to the primitives (`f64::floor`, which `core` leaves out). So the guest's
std has std's own sources for them:

- collections/hash/{map,set}.rs: HashMap and HashSet, over hashbrown;
- num/{f32,f64}.rs: the float methods, over `core`'s math and libm's functions (sys/cmath.rs),
  which the kernel's link needs only if something calls them.

Tests and stability attributes, which only std may use, are dropped. Run it again after changing
the toolchain.
"""

import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
GUEST = ROOT / "guest" / "rust"

HEADER = """// Rust's `library/std/src/{name}` (MIT OR Apache-2.0), as scripts/import-guest-std.py imports
// it into the guest kernel's std from the pinned toolchain's rust-src: without its tests and
// stability attributes. Do not edit it here.

"""

# The files imported: std's path, and the guest std's.
FILES = (
    ("collections/hash/map.rs", "collections/hash/map.rs"),
    ("collections/hash/set.rs", "collections/hash/set.rs"),
    ("num/f32.rs", "f32.rs"),
    ("num/f64.rs", "f64.rs"),
    ("sys/cmath.rs", "sys/cmath.rs"),
)


def sysroot() -> pathlib.Path:
    out = subprocess.run(
        ["rustc", "--print", "sysroot"], cwd=GUEST, check=True, capture_output=True, text=True
    )
    return pathlib.Path(out.stdout.strip())


def drop_attributes(text: str, names: tuple) -> str:
    """Removes the attributes `#[name...]` for each of `names`, however many lines they take,
    with the line they leave blank if they were alone on it."""
    out = []
    i = 0
    pattern = re.compile(r"#\[(" + "|".join(names) + r")\b")
    while True:
        match = pattern.search(text, i)
        if not match:
            out.append(text[i:])
            break
        start = match.start()
        # The attribute ends at the bracket that balances its opening one.
        depth = 0
        j = start + 1
        while True:
            c = text[j]
            if c == "[" or c == "(":
                depth += 1
            elif c == "]" or c == ")":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        end = j + 1
        line_start = text.rfind("\n", 0, start) + 1
        line_end = text.find("\n", end)
        if text[line_start:start].strip() == "" and text[end:line_end].strip() == "":
            out.append(text[i:line_start])
            i = line_end + 1
        else:
            out.append(text[i:start])
            i = end
            while i < len(text) and text[i] == " ":
                i += 1
    return "".join(out)


def convert(text: str, name: str) -> str:
    text = text.replace("#[cfg(test)]\nmod tests;\n", "")
    # Inner attributes of std's modules.
    text = re.sub(r"^#!\[(stable|unstable|cfg\(not\(test\)\))[^\n]*\]\n", "", text, flags=re.M)
    text = drop_attributes(
        text,
        (
            "stable",
            "unstable",
            # But for rustc_allow_incoherent_impl, which lets std add methods to primitives.
            "rustc_(?!allow_incoherent_impl)[a-z_]+",
            "cfg_attr\\(not\\(test\\), rustc_diagnostic_item",
            "cfg_attr\\(not\\(test\\), rustc_[a-z_]+",
        ),
    )
    return HEADER.format(name=name) + text


def main() -> int:
    source = sysroot() / "lib/rustlib/src/rust/library/std/src"
    target = GUEST / "std/src"
    for src, dst in FILES:
        (target / dst).parent.mkdir(parents=True, exist_ok=True)
        (target / dst).write_text(convert((source / src).read_text(), src))
    return 0


if __name__ == "__main__":
    sys.exit(main())
