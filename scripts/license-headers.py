#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Checks that each of the repo's source files carries the license header its origin needs.

- Code copied or adapted from Fuchsia keeps The Fuchsia Authors' copyright: a file that has it, or
  that cites a Fuchsia source in its leading comment, as scripts/upstream-changes.py finds them.
- fxdax's own code, and its adaptations, carry The fxdax Authors' copyright.
- Either is under the license of the nearest LICENSE file above it: BSD-3-Clause at the root, as
  Fuchsia is, MIT in crates/physmem, lifted from fxdax's port of the Zircon kernel, as Zircon is. The
  header says which, in Fuchsia's words.
- guest/linux, built into the guest's Linux kernel, is GPL-2.0 OR BSD-3-Clause, and its patches to
  the kernel GPL-2.0, as the kernel is: an SPDX line, then fxdax's copyright.
- The parts of Rust's std imported into the guest's std (scripts/import-guest-std.py) keep Rust's
  copyright and its MIT OR Apache-2.0 license.
- libkrun's test framework, which the guest tests' harness (guest/tests) came from, keeps
  libkrun's copyright and its Apache-2.0 license (guest/tests/LICENSE.libkrun), with fxdax's
  copyright for its changes; the test cases fxdax wrote there are fxdax's own.

Usage: scripts/license-headers.py [--fix]

--fix   add the headers missing: fxdax's, and Fuchsia's to a file citing Fuchsia, of the year of the
        oldest copyright among the sources it cites, read from the Fuchsia checkout ($FUCHSIA, by
        default ~/src/fuchsia) at the revision the root Cargo.toml records
"""

import argparse
import importlib.util
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

spec = importlib.util.spec_from_file_location("upstream", ROOT / "scripts" / "upstream-changes.py")
upstream = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upstream)

FUCHSIA = re.compile(r"Copyright \d{4} The Fuchsia Authors")
FXDAX = re.compile(r"Copyright \d{4} The fxdax Authors")
RUST = re.compile(r"Copyright \(c\) The Rust Project Contributors")
LIBKRUN = re.compile(r"Copyright The libkrun Authors")
STYLE = {
    "BSD": re.compile(r"governed by a BSD-style license"),
    "MIT": re.compile(r"governed by a MIT-style\s+(//|#)?\s*license"),
}
SPDX = re.compile(r"SPDX-License-Identifier: (.+?)\s*(\*/)?$", re.MULTILINE)
YEAR = re.compile(r"Copyright (\d{4}) The Fuchsia Authors")

# The files with headers: code, and the guest kernel's build files.
SUFFIXES = (".rs", ".c", ".h", ".sh", ".py", ".patch")
BUILD_FILES = ("Makefile", "Kconfig")


def git(*args, cwd=ROOT):
    return subprocess.run(
        ["git", "-C", str(cwd), *args], check=True, capture_output=True, text=True
    ).stdout


def files():
    for path in git("ls-files").splitlines():
        name = path.rsplit("/", 1)[-1]
        if (path.endswith(SUFFIXES) or name in BUILD_FILES) and (ROOT / path).is_file():
            yield path


def marker(path):
    return "//" if path.endswith((".rs", ".c", ".h")) else "#"


def leading(path, text):
    """A file's leading comment: its first lines, past a shebang, until the first line of code."""
    if path.endswith(".patch"):
        # The text before the diff.
        return text.split("\ndiff ", 1)[0]
    lines = []
    for line in text.splitlines():
        if line.startswith("#!") and not lines:
            continue
        if line and not line.lstrip().startswith(("//", "/*", "*", "#")):
            break
        lines.append(line)
    return "\n".join(lines)


def style(path):
    """The license of the nearest LICENSE file above `path`: "MIT" or "BSD"."""
    directory = (ROOT / path).parent
    while True:
        license = directory / "LICENSE"
        if license.exists():
            return "MIT" if "Permission is hereby granted" in license.read_text() else "BSD"
        directory = directory.parent


def needs(path, text):
    """What `path`'s header needs, as (kind, details)."""
    if path.startswith("guest/linux/"):
        return ("spdx", "GPL-2.0" if path.endswith(".patch") else "GPL-2.0 OR BSD-3-Clause")
    head = leading(path, text)
    if RUST.search(head) or "Rust's `library/" in head:
        return ("rust", None)
    if LIBKRUN.search(head):
        return ("libkrun", None)
    cited = path.startswith("crates/") and upstream.CITATION.findall(head)
    if not cited:
        return ("fuchsia" if FUCHSIA.search(head) else "fxdax", style(path))
    # The license of what it ports, if its sources agree on one: Zircon's kernel is MIT, the rest
    # of Fuchsia BSD.
    styles = upstream_copyrights(cited)[2]
    return ("fuchsia", next(iter(styles)) if len(styles) == 1 else style(path))


def problems(path, text):
    head = leading(path, text)
    kind, detail = needs(path, text)
    found = []
    if kind == "spdx":
        spdx = SPDX.search(head)
        if not spdx or spdx.group(1) != detail:
            found.append(f"no SPDX-License-Identifier: {detail}")
        if not FXDAX.search(head):
            found.append("no copyright of The fxdax Authors")
        return found
    if kind == "rust":
        spdx = SPDX.search(head)
        if not spdx or spdx.group(1) != "MIT OR Apache-2.0" or not RUST.search(head):
            found.append("not Rust's copyright and license (scripts/import-guest-std.py)")
        return found
    if kind == "libkrun":
        spdx = SPDX.search(head)
        if not spdx or spdx.group(1) != "Apache-2.0":
            found.append("libkrun's, without SPDX-License-Identifier: Apache-2.0")
        if not FXDAX.search(head):
            found.append("libkrun's, without The fxdax Authors' copyright of its changes")
        return found
    if kind == "fuchsia":
        if not FUCHSIA.search(head):
            found.append("cites Fuchsia, without The Fuchsia Authors' copyright")
        cited = upstream.CITATION.findall(head)
        for other in upstream_copyrights(cited)[1] if cited else []:
            if other not in head:
                found.append(f"cites Fuchsia sources of \"{other}\", without that copyright")
    if kind == "fxdax" and not FXDAX.search(head):
        found.append("no copyright of The fxdax Authors")
    if not STYLE[detail].search(head):
        found.append(f"no {detail}-style license notice, as {why(path, kind, detail)}")
    other = "MIT" if detail == "BSD" else "BSD"
    if STYLE[other].search(head):
        found.append(f"an {other}-style license notice, but {why(path, kind, detail)}")
    return found


def why(path, kind, detail):
    """Why `path`'s license is `detail`."""
    if kind == "fuchsia" and upstream.CITATION.findall(leading(path, (ROOT / path).read_text())):
        return f"the Fuchsia sources it cites are {detail}"
    return f"{style_file(path)} is {detail}"


def style_file(path):
    directory = (ROOT / path).parent
    while not (directory / "LICENSE").exists():
        directory = directory.parent
    return str((directory / "LICENSE").relative_to(ROOT))


def created(path):
    """The year `path` was first committed."""
    years = git("log", "--follow", "--diff-filter=A", "--format=%ad", "--date=format:%Y", "--", path)
    return years.split()[-1] if years.split() else "2026"


_upstream = {}


def upstream_copyrights(citations):
    """The Fuchsia sources' copyrights: the year of the oldest of The Fuchsia Authors', the others
    of the files cited, such as Zircon's of LK's authors; and the styles of their licenses."""
    key = tuple(sorted(set(citations)))
    if key in _upstream:
        return _upstream[key]
    commit = upstream.recorded_commit()
    years, others, styles = [], [], set()
    for citation in key:
        kind = subprocess.run(
            ["git", "-C", str(upstream.FUCHSIA), "cat-file", "-t", f"{commit}:{citation}"],
            capture_output=True, text=True,
        ).stdout.strip()
        if kind not in ("blob", "tree"):
            sys.exit(f"`{citation}` is not in Fuchsia at {commit} ({upstream.FUCHSIA})")
        sources = [citation]
        if kind == "tree":
            listed = git("ls-tree", "-r", "--name-only", commit, citation, cwd=upstream.FUCHSIA)
            sources = [s for s in listed.splitlines() if s.endswith(
                (".rs", ".cc", ".h", ".fidl", ".c", ".gn")
            )][:50]
        for source in sources:
            shown = subprocess.run(
                ["git", "-C", str(upstream.FUCHSIA), "show", f"{commit}:{source}"],
                capture_output=True, text=True, errors="replace",
            ).stdout[:2000]
            years += YEAR.findall(shown)
            styles |= {style for style, notice in STYLE.items() if notice.search(shown)}
            # Others' copyrights, of the files cited: not of every file of a directory cited.
            if kind == "blob":
                for line in re.findall(r"Copyright \(c\)[^\n]*", shown):
                    line = line.strip().rstrip(".")
                    if line not in others:
                        others.append(line)
    if not years:
        sys.exit(f"no Fuchsia copyright found in {list(key)} at {commit}")
    _upstream[key] = (min(years), others, styles)
    return _upstream[key]


def notice(license, prefix, path):
    """The notice of `license`, "MIT" or "BSD", for `path`."""
    if license == "MIT":
        return [
            f"{prefix}",
            f"{prefix} Use of this source code is governed by a MIT-style",
            f"{prefix} license that can be found in the LICENSE file or at",
            f"{prefix} https://opensource.org/licenses/MIT",
        ]
    # Under a directory whose LICENSE is another, the root's.
    where = "the LICENSE file" if style(path) == "BSD" else "the LICENSE file at the repo's root"
    return [
        f"{prefix} Use of this source code is governed by a BSD-style license that can be",
        f"{prefix} found in {where}.",
    ]


def missing(path, text):
    """What `path`'s header lacks: copyright lines, and the license notice, if it lacks it."""
    head = leading(path, text)
    kind, detail = needs(path, text)
    prefix = marker(path)
    if kind == "spdx":
        copyright = f"Copyright {created(path)} The fxdax Authors."
        spdx = f"SPDX-License-Identifier: {detail}"
        wrap = (lambda line: f"/* {line} */") if path.endswith(".h") else (
            lambda line: f"{prefix} {line}"
        )
        lines = [] if SPDX.search(head) else [wrap(spdx)]
        return lines + ([] if FXDAX.search(head) else [wrap(copyright)]), []
    if kind in ("rust", "libkrun"):
        return [], []
    reserved = "" if detail == "MIT" else " All rights reserved."
    lines = []
    if kind == "fuchsia":
        year, others, _ = upstream_copyrights(upstream.CITATION.findall(head))
        if not FUCHSIA.search(head):
            lines.append(f"{prefix} Copyright {year} The Fuchsia Authors.{reserved}")
            # fxdax adapted what it cites, as it carries no header of Fuchsia's.
            if not FXDAX.search(head):
                lines.append(f"{prefix} Copyright {created(path)} The fxdax Authors.{reserved}")
        lines += [f"{prefix} {other}" for other in others if other not in head]
    elif not FXDAX.search(head):
        lines.append(f"{prefix} Copyright {created(path)} The fxdax Authors.{reserved}")
    return lines, [] if STYLE[detail].search(head) else notice(detail, prefix, path)


def fix(path, text):
    copyrights, notice_lines = missing(path, text)
    if not copyrights and not notice_lines:
        return text
    lines = text.splitlines(keepends=True)
    added = "".join(line + "\n" for line in copyrights + notice_lines)
    head_len = len(leading(path, text).splitlines()) + (1 if text.startswith("#!") else 0)
    existing = [at for at in range(min(head_len, len(lines))) if "Copyright" in lines[at]]
    if existing:
        # After the copyrights the file has.
        at = existing[-1] + 1
        return "".join(lines[:at]) + added + "".join(lines[at:])
    at = 0
    # Past a shebang, shellcheck's directives for the whole file, and an SPDX line.
    while at < len(lines) and (
        (at == 0 and lines[at].startswith("#!"))
        or lines[at].startswith("# shellcheck shell=")
        or SPDX.search(lines[at])
    ):
        at += 1
    blank = "" if path.startswith("guest/linux/") else "\n"
    return "".join(lines[:at]) + added + blank + "".join(lines[at:])


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--fix", action="store_true")
    args = parser.parse_args()
    failed = 0
    for path in files():
        text = (ROOT / path).read_text(errors="replace")
        found = problems(path, text)
        if found and args.fix:
            text = fix(path, text)
            (ROOT / path).write_text(text)
            found = problems(path, text)
        for problem in found:
            print(f"{path}: {problem}")
            failed = 1
    return failed


if __name__ == "__main__":
    sys.exit(main())
