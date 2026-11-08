// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// What the guide's tools know of the repo: where it is, where its code comes from, and git.

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { Origin } from "../../src/data/types.ts";

/** The repo's root: the guide lives in its `guide/`, unless FXDAX_REPO names another. */
export const REPO = resolve(
  process.env.FXDAX_REPO ?? join(dirname(fileURLToPath(import.meta.url)), "..", "..", ".."),
);

/** The guide's own directory. */
export const GUIDE = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

/**
 * Where code comes from, as README's "Where the code comes from" sorts it:
 * - `fxdax`: fxdax's own;
 * - `ported`: fxdax's own code citing a Fuchsia source it ports or excerpts;
 * - `fuchsia`: copied from Fuchsia, as upstream wrote it;
 * - `fuchsia-changed`: copied from Fuchsia, deviating from upstream as its leading comment says;
 * - `stand-in`: fxdax's stand-in for a crate only Fuchsia's build has.
 */
export type { Origin };

export const ORIGINS: readonly Origin[] = [
  "fxdax",
  "ported",
  "fuchsia-changed",
  "fuchsia",
  "stand-in",
];

/** A Fuchsia source cited, as scripts/upstream-changes.py's CITATION reads them. */
export const CITATION = /`((?:zircon|src|sdk|third_party|prebuilt)\/[^`\s]+)`/;

/** The stand-in crates of README's table "`crates/`: stand-ins for Fuchsia-only pieces". */
export function standIns(repo = REPO): Set<string> {
  const readme = readFileSync(join(repo, "README.md"), "utf8");
  const section = readme.split("### `crates/`: stand-ins for Fuchsia-only pieces")[1];
  if (section === undefined) throw new Error("README has no stand-ins section");
  const table = section.split("\n\n").find((part) => part.trimStart().startsWith("|"));
  if (table === undefined) throw new Error("README's stand-ins section has no table");
  const names = new Set<string>();
  for (const row of table.split("\n").slice(2)) {
    const cell = row.split("|")[1] ?? "";
    // `fidl-fuchsia-io`, `-fs`, `-fxfs`: a suffix names a crate of the first's prefix.
    let prefix = "";
    for (const [, name] of cell.matchAll(/`([^`]+)`/g)) {
      if (name.startsWith("-") && prefix) names.add(prefix + name.slice(1));
      else {
        names.add(name);
        prefix = name.slice(0, name.lastIndexOf("-") + 1);
      }
    }
  }
  // zx-sys has no row: it is the syscalls fxdax defines none of, a stand-in all the same.
  if (existsSync(join(repo, "crates", "zx-sys"))) names.add("zx-sys");
  return names;
}

/** The comments before a file's first line of code. */
export function leadingComment(text: string): string {
  const lines: string[] = [];
  for (const line of text.split("\n")) {
    const trimmed = line.trimStart();
    if (line && !(trimmed.startsWith("//") || trimmed.startsWith("/*") || trimmed.startsWith("*")))
      break;
    lines.push(line);
  }
  return lines.join("\n");
}

/** Each crate under crates/ by its origin, from its Cargo.toml's citation of Fuchsia. */
export function crateOrigins(repo = REPO): Map<string, Origin> {
  const stand = standIns(repo);
  const origins = new Map<string, Origin>();
  for (const name of readdirSync(join(repo, "crates")).sort()) {
    const manifest = join(repo, "crates", name, "Cargo.toml");
    if (!existsSync(manifest)) continue;
    if (stand.has(name)) origins.set(name, "stand-in");
    else origins.set(name, CITATION.test(readFileSync(manifest, "utf8")) ? "fuchsia" : "fxdax");
  }
  return origins;
}

/**
 * A file's origin: its crate's, refined by its leading comment. In a crate copied from Fuchsia,
 * a file under fxdax's copyright is fxdax's own, and one under Fuchsia's whose leading comment
 * names fxdax deviates from upstream; in fxdax's crates, a file citing a Fuchsia source ports it.
 */
export function fileOrigin(crate: Origin, text: string): Origin {
  const head = leadingComment(text);
  if (crate === "fuchsia") {
    if (head.includes("The fxdax Authors")) return "fxdax";
    return head.includes("fxdax") ? "fuchsia-changed" : "fuchsia";
  }
  if (crate === "fxdax" && CITATION.test(head)) return "ported";
  return crate;
}

/** The origin of the file at `path`, relative to the repo. */
export function originOf(path: string, crates: Map<string, Origin>, repo = REPO): Origin {
  const match = /^crates\/([^/]+)\//.exec(path);
  const crate = match ? (crates.get(match[1]) ?? "fxdax") : "fxdax";
  return fileOrigin(crate, readFileSync(join(repo, path), "utf8"));
}

export function git(args: string[], cwd = REPO): string {
  return execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
}

/** The repo's commit, and the Firecracker submodule's, which links into its own repo name. */
export function revisions(repo = REPO): { commit: string; firecracker: string } {
  return {
    commit: git(["rev-parse", "HEAD"], repo),
    firecracker: git(["rev-parse", "HEAD"], join(repo, "vendor", "firecracker")),
  };
}
