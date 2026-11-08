// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// What must hold of the guide's content against the repo, or the build fails: every anchor's
// snippet occurs exactly once in its file; every piece of evidence an invariant cites exists; every
// mutant and trace the missions name exists; every doc a layer names exists.

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import type { Anchor } from "../../src/content/anchors.ts";
import type { EvidenceRef, Invariant, Layer } from "../../src/content/types.ts";
import type { Evidence } from "../../src/data/types.ts";
import { occurrences } from "./symbols.ts";

export interface Content {
  anchors: Record<string, Anchor>;
  invariants: Invariant[];
  layers: Layer[];
  /** Mutant files the missions name, by where. */
  mutants: Record<string, string[]>;
  /** Traces the missions replay: SPEC/NAME. */
  traces: string[];
}

export function checkContent(repo: string, guide: string, content: Content, evidence: Evidence): string[] {
  const problems: string[] = [];
  for (const [id, anchor] of Object.entries(content.anchors)) {
    const file = join(repo, anchor.path);
    if (!existsSync(file)) {
      problems.push(`anchor ${id}: ${anchor.path} does not exist`);
      continue;
    }
    const count = occurrences(readFileSync(file, "utf8"), anchor.snippet);
    if (count !== 1) {
      problems.push(`anchor ${id}: its snippet occurs ${count} times in ${anchor.path}, not once: ${anchor.snippet}`);
    }
  }
  const known = (ref: EvidenceRef): boolean => {
    switch (ref.kind) {
      case "doc":
        return evidence.docs.some((d) => d.id === ref.id && (!ref.heading || d.headings.includes(ref.heading)));
      case "deadEnd":
        return evidence.deadEnds.some((d) => d.file === ref.file);
      case "mutant":
        return evidence.mutants.some((m) => m.file === ref.file);
      case "mutation":
        return evidence.mutations.some((m) => m.spec === ref.spec && m.name === ref.name);
    }
  };
  for (const inv of content.invariants) {
    for (const id of inv.enforcedBy) {
      if (!(id in content.anchors)) problems.push(`invariant ${inv.id}: no anchor ${id}`);
    }
    for (const ref of inv.evidence) {
      if (!known(ref)) problems.push(`invariant ${inv.id}: no such ${ref.kind}: ${JSON.stringify(ref)}`);
    }
    const checks = inv.evidence.some((r) => r.kind === "mutation" || r.kind === "mutant");
    if ((inv.standing === "checked") !== checks) {
      problems.push(`invariant ${inv.id}: is "${inv.standing}" but ${checks ? "cites" : "cites no"} mutation or mutant`);
    }
  }
  for (const layer of content.layers) {
    for (const id of layer.anchors) {
      if (!(id in content.anchors)) problems.push(`layer ${layer.id}: no anchor ${id}`);
    }
    for (const doc of layer.docs) {
      if (!evidence.docs.some((d) => d.id === doc)) problems.push(`layer ${layer.id}: no doc ${doc}`);
    }
    for (const dir of layer.dirs) {
      if (!existsSync(join(repo, dir))) problems.push(`layer ${layer.id}: ${dir} does not exist`);
    }
  }
  for (const [where, files] of Object.entries(content.mutants)) {
    for (const file of files) {
      if (!evidence.mutants.some((m) => m.file === file)) problems.push(`${where}: no mutant ${file}`);
    }
  }
  for (const trace of content.traces) {
    if (!existsSync(join(guide, "fixtures", "traces", `${trace}.json`))) {
      problems.push(`trace ${trace} is not recorded: run pnpm traces ${trace}`);
    }
  }
  return problems;
}
