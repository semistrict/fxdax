// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Copies the local source the guide shows into src/generated/public/source/, which Vite serves
// as is (publicDir): every file the index covers, every anchor's file, the TLA+ specs and their
// models, the design docs and the dead ends. The guide reads code from here, never from a website.
// Usage: node tools/sources.ts

import { copyFileSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { anchors } from "../src/content/anchors.ts";
import type { SymbolIndex } from "../src/data/types.ts";
import { GUIDE, REPO } from "./lib/repo.ts";

const out = join(GUIDE, "src", "generated", "public", "source");
rmSync(out, { recursive: true, force: true });

function* walk(dir: string, keep: (path: string) => boolean): Generator<string> {
  for (const entry of readdirSync(join(REPO, dir)).sort()) {
    const path = join(dir, entry);
    if (statSync(join(REPO, path)).isDirectory()) yield* walk(path, keep);
    else if (keep(path)) yield path;
  }
}

const index = JSON.parse(readFileSync(join(GUIDE, "src", "generated", "symbols.json"), "utf8")) as SymbolIndex;
const paths = new Set<string>([
  ...index.files.map(([path]) => path),
  ...Object.values(anchors).map((a) => a.path),
  ...walk("specs", (p) => p.endsWith(".tla") || p.endsWith(".cfg")),
  ...walk("backlog/docs", (p) => p.endsWith(".md")),
  ...walk("dead-ends", (p) => p.endsWith(".md")),
]);
let bytes = 0;
for (const path of paths) {
  const to = join(out, path);
  mkdirSync(dirname(to), { recursive: true });
  copyFileSync(join(REPO, path), to);
  bytes += statSync(to).size;
}
writeFileSync(join(out, "..", "sources.json"), JSON.stringify([...paths].sort()));
// The same files in one object, for the single-file build (src/data/links.ts).
const inline: Record<string, string> = {};
for (const path of [...paths].sort()) inline[path] = readFileSync(join(REPO, path), "utf8");
writeFileSync(join(GUIDE, "src", "generated", "sources-inline.json"), JSON.stringify(inline));
console.log(`${paths.size} files of local source, ${(bytes / 1e6).toFixed(1)} MB, in ${relative(GUIDE, out)}`);
