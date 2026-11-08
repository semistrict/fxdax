// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Writes src/generated/evidence.json: the repo's mutants, specs, mutations, dead ends and docs.
// Usage: node tools/evidence.ts

import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { readEvidence } from "./lib/evidence.ts";
import { GUIDE } from "./lib/repo.ts";

const evidence = readEvidence();
const out = join(GUIDE, "src", "generated");
mkdirSync(out, { recursive: true });
writeFileSync(join(out, "evidence.json"), JSON.stringify(evidence));
console.log(
  `${evidence.mutants.length} mutants, ${evidence.specs.length} specs, ` +
    `${evidence.mutations.length} mutations, ${evidence.deadEnds.length} dead ends, ` +
    `${evidence.docs.length} docs`,
);
