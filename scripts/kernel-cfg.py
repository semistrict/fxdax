#!/usr/bin/env python3
"""Generates copies of vendored sources that build their Fuchsia code paths on the in-process kernel.

Some vendored code only builds what fxdax needs from it, such as VMO-backed block buffers, when
`target_os = "fuchsia"`. For each source listed in SOURCES, a crate directory or a single file, this
mirrors it from vendor/fuchsia to vendor/fuchsia-kernel, rewriting `target_os = "fuchsia"` in cfg
conditions to the `fxdax_kernel` cfg, which the crate's build script sets. Nothing else changes, and
files the rewrite leaves as they are become symlinks to vendor/fuchsia.

Each source's rules choose what is rewritten, matching patterns (in multi-line mode) against items,
bodies included and indentation removed:

- `include`: if given, only matching items' Fuchsia code paths are built;
- `exclude`: matching items' Fuchsia code paths, such as ones needing FIDL protocols, stay off;
- `disable`: matching items, attributes included, are compiled out on the kernel by a
  `#[cfg(not(fxdax_kernel))]` attribute, such as tests that need Fuchsia's component framework.
  Each pattern must match an item.

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
    "src/storage/fxfs/platform/src/fuchsia/pager.rs": Rules(
        disable=[
            # The panics these expect are in packet receivers, which abort the process, as on
            # Fuchsia, where the test runner checks the message a test process dies with; and the
            # helper only they use.
            r'^#\[should_panic\(expected = "PagerRange(ChunksIter)? was dropped',
            r"^fn partial_supply_pages\(",
            # Expects 4 KiB pages.
            r"^async fn test_pager_range_bad_expand_panics\(",
            # Need VMO children, which the kernel doesn't have: fxdax maps files' VMOs into guests
            # itself rather than handing out children to map.
            r"^async fn test_watch_for_zero_children\(",
            r"^async fn test_multiple_watch_for_zero_children_calls\(",
            # And what only they use.
            r"^use futures::StreamExt;",
            r"^use futures::channel::mpsc;",
            r"^struct OnZeroChildrenFile\b",
            r"^impl\b[^{]*\bOnZeroChildrenFile\b",
        ],
    ),
}

FUCHSIA = 'target_os = "fuchsia"'
KERNEL = "fxdax_kernel"
CFG_ATTRIBUTE = re.compile(r"^\s*#\[cfg(_attr)?\(")
CFG_MACRO = re.compile(r"cfg!\(")
# The first line of an item without attributes.
ITEM = re.compile(
    r'^\s*(pub(\([^)]*\))?\s+)?((async|unsafe|const|extern(\s+"[^"]*")?)\s+)*'
    r"(fn|struct|enum|union|mod|impl|trait|type|static|const|use)\b"
)


def is_attribute(line):
    return line.lstrip().startswith("#[")


def item_text(lines, start, end):
    """Returns `lines[start:end]` without the indentation of the first."""
    indent = len(lines[start]) - len(lines[start].lstrip()) if start < end else 0
    return "".join(line[indent:] for line in lines[start:end])


def item_start(lines, index):
    """Returns the index of the first line after `index` that isn't an attribute or comment."""
    start = index
    while start < len(lines):
        stripped = lines[start].strip()
        if not (stripped.startswith("#[") or stripped.startswith("//") or not stripped):
            break
        start += 1
    return start


def item_end(lines, start):
    """Returns the index after the last line of the item starting at `start`.

    The item runs to the first `;` outside braces or the brace closing its body; braces in strings
    and comments are not special-cased, which the vendored code doesn't need.
    """
    depth = 0
    for number in range(start, len(lines)):
        for char in lines[number]:
            depth += {"{": 1, "}": -1}.get(char, 0)
            if depth == 0 and char in "};":
                return number + 1
    return len(lines)


def matches(patterns, item):
    return [pattern for pattern in patterns if re.search(pattern, item, re.MULTILINE)]


def rewrite(text, rules, disabled):
    """Rewrites `text`, adding the `disable` patterns that matched to `disabled`."""
    lines = text.splitlines(keepends=True)
    out = []
    for index, line in enumerate(lines):
        follows_attribute = index > 0 and is_attribute(lines[index - 1])
        if (is_attribute(line) or ITEM.match(line)) and not follows_attribute:
            start = item_start(lines, index)
            matched = matches(rules.disable, item_text(lines, index, item_end(lines, start)))
            if matched:
                disabled.update(matched)
                indent = line[: len(line) - len(line.lstrip())]
                out.append(f"{indent}#[cfg(not({KERNEL}))]\n")
        attribute = CFG_ATTRIBUTE.match(line)
        if FUCHSIA in line and (attribute or CFG_MACRO.search(line)):
            if attribute:
                start = item_start(lines, index + 1)
                item = item_text(lines, start, item_end(lines, start))
            else:
                item = line
            included = rules.include is None or matches(rules.include, item)
            if included and not matches(rules.exclude, item):
                line = line.replace(FUCHSIA, KERNEL)
        out.append(line)
    return "".join(out)


def generate():
    """Returns the generated files, as a map from path to contents, or to None for a symlink."""
    files = {}
    for relative, rules in SOURCES.items():
        disabled = set()
        source = SOURCE / relative
        paths = sorted(p for p in source.rglob("*") if p.is_file()) if source.is_dir() else [source]
        for path in paths:
            dest = DEST / path.relative_to(SOURCE)
            files[dest] = None
            if path.suffix == ".rs":
                text = path.read_text()
                rewritten = rewrite(text, rules, disabled)
                if rewritten != text:
                    files[dest] = rewritten
        for pattern in set(rules.disable) - disabled:
            sys.exit(f"{relative}: no item with attributes matches disable pattern {pattern!r}")
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
