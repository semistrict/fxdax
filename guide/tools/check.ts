// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Fails if the guide's content no longer matches the repo (lib/check.ts): a snippet moved or
// repeated, evidence gone, a trace not recorded. Usage: node tools/check.ts

import { anchors } from "../src/content/anchors.ts";
import { invariants } from "../src/content/invariants.ts";
import { layers } from "../src/content/layers.ts";
import { ruleMutants } from "../src/missions/pagePilot/rules.ts";
import { checkContent } from "./lib/check.ts";
import { readEvidence } from "./lib/evidence.ts";
import { GUIDE, REPO } from "./lib/repo.ts";

const evidence = readEvidence();
const traces = evidence.mutations
  .filter((m) => m.spec === "dax-cow" && m.removed.length === 1)
  .map((m) => `${m.spec}/${m.name}`);
const problems = checkContent(REPO, GUIDE, { anchors, invariants, layers, mutants: { "Page Pilot's rules": Object.values(ruleMutants).flat() }, traces }, evidence);
if (problems.length) {
  console.error(`The guide's content no longer matches the repo:\n  ${problems.join("\n  ")}`);
  process.exit(1);
}
console.log(`${Object.keys(anchors).length} anchors, ${invariants.length} invariants, ${traces.length} traces: all match the repo`);
