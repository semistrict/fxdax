#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
"""What a guest source uses of the kernel, as the compiler sees it against the real headers.

  inventory.py KERNEL SOURCE...

Parses each SOURCE with libclang against a configured arm64 tree (KERNEL), and counts what it
spells itself: a name a macro expands to inside a header is the header's business, not the
source's, and is what the shim gets to choose. The first SOURCE's directory is on the include
path, so fs/fxfs's local headers count as the source's interface rather than as the kernel's.
"""

import collections
import glob
import os
import re
import sys

import clang.cindex as ci

ci.Config.set_library_file(glob.glob("/usr/lib/*/libclang-*.so.1")[0])

LOCAL = ("fxfs.h", "device.h", "vfs.h", "hist.h", "rt.h")


def flags(kernel, source):
    resource = os.popen("clang -print-resource-dir").read().strip()
    arch = f"{kernel}/arch/arm64/include"
    return [
        "--target=aarch64-linux-gnu", "-nostdinc", "-std=gnu11", "-D__KERNEL__",
        "-DKBUILD_MODNAME=\"fxfs\"", "-DKBUILD_BASENAME=\"dax\"",
        f"-isystem{resource}/include",
        f"-I{os.path.dirname(os.path.abspath(source))}",
        f"-I{arch}", f"-I{arch}/generated", f"-I{kernel}/include", f"-I{arch}/uapi",
        f"-I{arch}/generated/uapi", f"-I{kernel}/include/uapi", f"-I{kernel}/include/generated/uapi",
        "-include", f"{kernel}/include/linux/compiler-version.h",
        "-include", f"{kernel}/include/linux/kconfig.h",
        "-include", f"{kernel}/include/linux/compiler_types.h",
        "-Wno-everything", "-ferror-limit=0",
    ]


def spelled(source):
    """The identifiers `source` spells outside comments and strings."""
    text = open(source, encoding="utf-8").read()
    text = re.sub(r"/\*.*?\*/|//[^\n]*", " ", text, flags=re.S)
    text = re.sub(r'"(\\.|[^"\\])*"', " ", text)
    return set(re.findall(r"[A-Za-z_][A-Za-z0-9_]*", text))


def filename(location):
    return location.file.name if location.file else ""


def category(path):
    """Where a declaration lives: `local` (fs/fxfs), `self`, or `kernel`."""
    return "local" if os.path.basename(path) in LOCAL else "kernel"


def named_parent(field):
    """The struct a field is of, through the anonymous ones it may be nested in."""
    parent = field.semantic_parent
    while parent.is_anonymous() or "anonymous" in parent.spelling:
        parent = parent.semantic_parent
    return parent.spelling


def inventory(kernel, source):
    source = os.path.abspath(source)
    index = ci.Index.create()
    tu = index.parse(source, flags(kernel, source),
                     options=ci.TranslationUnit.PARSE_DETAILED_PROCESSING_RECORD)
    errors = [d for d in tu.diagnostics if d.severity >= ci.Diagnostic.Error]
    names = spelled(source)
    out = collections.defaultdict(set)
    own_macros = set()

    for cursor in tu.cursor.get_children():
        if filename(cursor.location) != source and cursor.kind == ci.CursorKind.MACRO_DEFINITION:
            continue
        if cursor.kind == ci.CursorKind.MACRO_DEFINITION:
            own_macros.add(cursor.spelling)
    for cursor in tu.cursor.get_children():
        if cursor.kind == ci.CursorKind.MACRO_INSTANTIATION and \
                filename(cursor.location) == source and cursor.spelling not in own_macros:
            out["macros"].add(cursor.spelling)
        if cursor.kind == ci.CursorKind.INCLUSION_DIRECTIVE and \
                filename(cursor.location) == source:
            out["includes"].add(cursor.spelling)

    def visit(cursor):
        kind = cursor.kind
        referenced = cursor.referenced
        if referenced is not None and kind in (ci.CursorKind.DECL_REF_EXPR,
                                               ci.CursorKind.CALL_EXPR,
                                               ci.CursorKind.MEMBER_REF_EXPR,
                                               ci.CursorKind.TYPE_REF):
            path = filename(referenced.location)
            name = referenced.spelling
            if path and path != source and name in names:
                where = category(path)
                rk = referenced.kind
                if rk == ci.CursorKind.FUNCTION_DECL:
                    out[f"{where}_functions"].add(name)
                elif rk == ci.CursorKind.FIELD_DECL:
                    out[f"{where}_fields"].add(f"{named_parent(referenced)}.{name}")
                elif rk == ci.CursorKind.ENUM_CONSTANT_DECL:
                    out[f"{where}_enum_constants"].add(name)
                elif rk == ci.CursorKind.VAR_DECL:
                    out[f"{where}_variables"].add(name)
                elif rk in (ci.CursorKind.STRUCT_DECL, ci.CursorKind.UNION_DECL):
                    out[f"{where}_structs"].add(name)
                elif rk == ci.CursorKind.TYPEDEF_DECL:
                    out[f"{where}_typedefs"].add(name)
                elif rk == ci.CursorKind.ENUM_DECL:
                    out[f"{where}_enums"].add(name)
        for child in cursor.get_children():
            visit(child)

    for cursor in tu.cursor.get_children():
        if filename(cursor.location) == source:
            visit(cursor)

    out["headers_parsed"] = {path.include.name for path in tu.get_includes()}
    return out, errors


def main():
    kernel, sources = sys.argv[1], sys.argv[2:]
    for source in sources:
        out, errors = inventory(kernel, source)
        print(f"== {source}: {len(errors)} parse errors")
        for error in errors[:10]:
            print(f"  {error}")
        for key in sorted(out):
            if key == "headers_parsed":
                print(f"  {key}: {len(out[key])}")
                continue
            print(f"  {key}: {len(out[key])}")
            print("    " + " ".join(sorted(out[key])))


if __name__ == "__main__":
    main()
