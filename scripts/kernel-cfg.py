#!/usr/bin/env python3
"""Generates copies of vendored sources that build their Fuchsia code paths on the in-process kernel.

Some vendored code only builds what fxdax needs from it, such as VMO-backed block buffers, when
`target_os = "fuchsia"`. For each source listed in SOURCES, a crate directory or a single file, this
mirrors it from vendor/fuchsia to vendor/fuchsia-kernel, rewriting `target_os = "fuchsia"` in cfg
conditions to the `fxdax_kernel` cfg, which the crate's build script sets. Nothing else changes, and
files the rewrite leaves as they are become symlinks to vendor/fuchsia.

Each source's rules choose what is rewritten, matching patterns against the item a cfg attribute
applies to, body included:

- `include`: if given, only matching items' Fuchsia code paths are built;
- `exclude`: matching items' Fuchsia code paths, such as ones needing FIDL protocols, stay off;
- `disable`: matching items are compiled out on the kernel, by adding `not(fxdax_kernel)` to their
  cfg attribute, such as tests that need Fuchsia's component framework.

Usage: kernel-cfg.py [--check]

With --check, fails if vendor/fuchsia-kernel is not what this would generate.
"""

import dataclasses
import os
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCE = ROOT / "vendor" / "fuchsia"
DEST = ROOT / "vendor" / "fuchsia-kernel"


@dataclasses.dataclass
class Rules:
    include: list = None
    exclude: list = dataclasses.field(default_factory=list)
    disable: list = dataclasses.field(default_factory=list)


# Sources, relative to vendor/fuchsia, and the rules for rewriting them.
SOURCES = {
    "src/storage/lib/storage_device/src": Rules(
        exclude=[
            # The FIDL block device and its mapper.
            r"\bblock_device\b",
            r"\bconnect_mapper\b",
            # Buffers pinned for DMA with BTIs.
            r"\bpinned_buffer_allocator\b",
        ],
    ),
    # The system page size.
    "src/storage/lib/units/src": Rules(),
    "src/storage/fxfs/src": Rules(
        include=[
            # The conversions from FxfsError to zx::Status.
            r"^mod fuchsia \{",
        ],
    ),
    "src/storage/fxfs/platform/src/fuchsia/paged_object_handle.rs": Rules(
        # The tests use FxFile through fuchsia.io connections.
        disable=[r"^mod tests \{"],
    ),
}

FUCHSIA = 'target_os = "fuchsia"'
KERNEL = "fxdax_kernel"
CFG_ATTRIBUTE = re.compile(r"^(\s*)#\[cfg(_attr)?\((.*)\)\]\s*$")
CFG_MACRO = re.compile(r"cfg!\(")


def gated_item(lines, index):
    """Returns the text of the item the attribute at `index` applies to, without its indentation.

    The item runs from its first line to the first `;` outside braces or the brace closing its
    body; braces in strings and comments are not special-cased, which the vendored code doesn't
    need.
    """
    start = index + 1
    while start < len(lines):
        stripped = lines[start].strip()
        if not (stripped.startswith("#[") or stripped.startswith("//") or not stripped):
            break
        start += 1
    end = item_end(lines, start)
    item = lines[start:end]
    indent = len(item[0]) - len(item[0].lstrip()) if item else 0
    return "".join(line[indent:] for line in item)


def item_end(lines, start):
    """Returns the index after the last line of the item starting at `start`."""
    depth = 0
    for number in range(start, len(lines)):
        for char in lines[number]:
            depth += {"{": 1, "}": -1}.get(char, 0)
            if depth == 0 and char in "};":
                return number + 1
    return len(lines)


def matches(patterns, item):
    return any(re.search(pattern, item) for pattern in patterns)


def rewrite(text, rules):
    lines = text.splitlines(keepends=True)
    out = []
    for index, line in enumerate(lines):
        attribute = CFG_ATTRIBUTE.match(line)
        item = gated_item(lines, index) if attribute else line
        if attribute and not attribute.group(2) and matches(rules.disable, item):
            indent, condition = attribute.group(1), attribute.group(3)
            line = f"{indent}#[cfg(all({condition}, not({KERNEL})))]\n"
        elif FUCHSIA in line and (attribute or CFG_MACRO.search(line)):
            included = rules.include is None or matches(rules.include, item)
            if included and not matches(rules.exclude, item):
                line = line.replace(FUCHSIA, KERNEL)
        out.append(line)
    return "".join(out)


def generate():
    """Returns the generated files, as a map from path to contents, or to None for a symlink."""
    files = {}
    for relative, rules in SOURCES.items():
        source = SOURCE / relative
        paths = sorted(p for p in source.rglob("*") if p.is_file()) if source.is_dir() else [source]
        for path in paths:
            dest = DEST / path.relative_to(SOURCE)
            files[dest] = None
            if path.suffix == ".rs":
                text = path.read_text()
                rewritten = rewrite(text, rules)
                if rewritten != text:
                    files[dest] = rewritten
    return files


def link_target(path):
    return os.path.relpath(SOURCE / path.relative_to(DEST), path.parent)


def current(path, contents):
    if contents is None:
        return path.is_symlink() and os.readlink(path) == link_target(path)
    return not path.is_symlink() and path.exists() and path.read_text() == contents


def main():
    check = "--check" in sys.argv[1:]
    files = generate()
    existing = set()
    if DEST.exists():
        existing = {p for p in DEST.rglob("*") if p.is_symlink() or p.is_file()}
    stale = sorted(existing - set(files))
    changed = sorted(path for path, contents in files.items() if not current(path, contents))
    if check:
        for path in stale + changed:
            print(f"{path.relative_to(ROOT)} is out of date; run scripts/kernel-cfg.py")
        sys.exit(1 if stale or changed else 0)
    for path in stale:
        path.unlink()
    for path in changed:
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.is_symlink() or path.exists():
            path.unlink()
        if files[path] is None:
            path.symlink_to(link_target(path))
        else:
            path.write_text(files[path])


if __name__ == "__main__":
    main()
