#!/usr/bin/env python3
"""Generates copies of vendored crates that build their Fuchsia code paths on the in-process kernel.

Some vendored crates only build what Fxfs needs from them, such as VMO-backed block buffers, when
`target_os = "fuchsia"`. For each crate listed in CRATES, this copies its sources from
vendor/fuchsia to vendor/fuchsia-kernel, rewriting `target_os = "fuchsia"` in cfg conditions to the
`fxdax_kernel` cfg, which the crate's build script sets. Nothing else changes.

A crate's Fuchsia paths that need services the kernel does not provide, such as FIDL protocols,
are left as they are: a cfg attribute is not rewritten if the item it applies to, body included,
matches one of the crate's exclusion patterns.

Usage: kernel-cfg.py [--check]

With --check, fails if vendor/fuchsia-kernel is not what this would generate.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCE = ROOT / "vendor" / "fuchsia"
DEST = ROOT / "vendor" / "fuchsia-kernel"

# Crate source directories, relative to vendor/fuchsia, and patterns matching the items whose
# Fuchsia code paths stay off.
CRATES = {
    "src/storage/lib/storage_device/src": [
        # The FIDL block device and its mapper.
        r"\bblock_device\b",
        r"\bconnect_mapper\b",
        # Buffers pinned for DMA with BTIs.
        r"\bpinned_buffer_allocator\b",
    ],
}

FUCHSIA = 'target_os = "fuchsia"'
KERNEL = "fxdax_kernel"
CFG_ATTRIBUTE = re.compile(r"^\s*#\[cfg(_attr)?\(")
CFG_MACRO = re.compile(r"cfg!\(")


def gated_item(lines, index):
    """Returns the text of the item the attribute at `index` applies to.

    The item runs from its first line to the first `;` outside braces or the brace closing its
    body; braces in strings and comments are not special-cased, which the vendored crates don't
    need.
    """
    start = index + 1
    while start < len(lines):
        stripped = lines[start].strip()
        if not (stripped.startswith("#[") or stripped.startswith("//") or not stripped):
            break
        start += 1
    depth = 0
    for end in range(start, len(lines)):
        for char in lines[end]:
            if char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
                if depth == 0:
                    return "".join(lines[start : end + 1])
            elif char == ";" and depth == 0:
                return "".join(lines[start : end + 1])
    return "".join(lines[start:])


def rewrite(text, exclusions):
    lines = text.splitlines(keepends=True)
    out = []
    for index, line in enumerate(lines):
        if FUCHSIA in line and (CFG_ATTRIBUTE.match(line) or CFG_MACRO.search(line)):
            item = gated_item(lines, index) if CFG_ATTRIBUTE.match(line) else line
            if not any(re.search(pattern, item) for pattern in exclusions):
                line = line.replace(FUCHSIA, KERNEL)
        out.append(line)
    return "".join(out)


def generate():
    """Returns the generated files, as a map from path to contents."""
    files = {}
    for crate, exclusions in CRATES.items():
        for source in sorted((SOURCE / crate).rglob("*.rs")):
            relative = source.relative_to(SOURCE)
            files[DEST / relative] = rewrite(source.read_text(), exclusions)
    return files


def main():
    check = "--check" in sys.argv[1:]
    files = generate()
    existing = {path for path in DEST.rglob("*.rs")} if DEST.exists() else set()
    stale = sorted(existing - set(files))
    changed = sorted(path for path, text in files.items()
                     if not path.exists() or path.read_text() != text)
    if check:
        for path in stale + changed:
            print(f"{path.relative_to(ROOT)} is out of date; run scripts/kernel-cfg.py")
        sys.exit(1 if stale or changed else 0)
    for path in stale:
        path.unlink()
    for path in changed:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(files[path])


if __name__ == "__main__":
    main()
