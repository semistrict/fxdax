// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Writes src/generated/symbols.json: every symbol of the repo's code (lib/symbols.ts), compact,
// files once each. Usage: node tools/index.ts

import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { SymbolIndex } from "../src/data/types.ts";
import { GUIDE, ORIGINS, revisions } from "./lib/repo.ts";
import { indexRepo } from "./lib/symbols.ts";

const symbols = await indexRepo();
const files: SymbolIndex["files"] = [];
const fileIndex = new Map<string, number>();
const rows: SymbolIndex["symbols"] = [];
for (const sym of symbols) {
  let at = fileIndex.get(sym.path);
  if (at === undefined) {
    at = files.length;
    fileIndex.set(sym.path, at);
    files.push([sym.path, sym.origin]);
  }
  rows.push([at, sym.kind, sym.name, sym.container, sym.signature, sym.doc, sym.unique ? 1 : 0]);
}
const index: SymbolIndex = { ...revisions(), files, symbols: rows };
const out = join(GUIDE, "src", "generated");
mkdirSync(out, { recursive: true });
writeFileSync(join(out, "symbols.json"), JSON.stringify(index));
writeFileSync(join(out, "meta.json"), JSON.stringify(revisions()));
const by = ORIGINS.map((o) => `${o} ${symbols.filter((s) => s.origin === o).length}`).join(", ");
console.log(`${symbols.length} symbols in ${files.length} files (${by})`);
