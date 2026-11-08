// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import { anchors } from "../src/content/anchors.ts";
import { invariants } from "../src/content/invariants.ts";
import { layers } from "../src/content/layers.ts";
import type { Evidence } from "../src/data/types.ts";
import { checkContent } from "../tools/lib/check.ts";
import { readEvidence } from "../tools/lib/evidence.ts";
import { GUIDE, REPO } from "../tools/lib/repo.ts";

const none: Evidence = { mutants: [], mutations: [], specs: [], deadEnds: [], docs: [] };

describe("the content check", () => {
  test("the guide's content matches the repo", () => {
    const evidence = readEvidence();
    const traces = evidence.mutations.filter((m) => m.spec === "dax-cow").map((m) => `${m.spec}/${m.name}`);
    expect(checkContent(REPO, GUIDE, { anchors, invariants, layers, mutants: {}, traces }, evidence)).toEqual([]);
  });

  test("an anchor whose snippet is missing, or repeated, fails it", () => {
    const repo = mkdtempSync(join(tmpdir(), "guide-check-"));
    mkdirSync(join(repo, "src"));
    writeFileSync(join(repo, "src", "a.rs"), "fn once() {}\nfn twice() {}\nfn twice() {}\n");
    const content = {
      anchors: {
        ok: { path: "src/a.rs", snippet: "fn once() {}", note: "" },
        twice: { path: "src/a.rs", snippet: "fn twice() {}", note: "" },
        gone: { path: "src/a.rs", snippet: "fn gone() {}", note: "" },
        nofile: { path: "src/b.rs", snippet: "x", note: "" },
      },
      invariants: [],
      layers: [],
      mutants: { here: ["nope.patch"] },
      traces: ["dax-cow/NoSuchModel"],
    };
    expect(checkContent(repo, repo, content, none)).toEqual([
      "anchor twice: its snippet occurs 2 times in src/a.rs, not once: fn twice() {}",
      "anchor gone: its snippet occurs 0 times in src/a.rs, not once: fn gone() {}",
      "anchor nofile: src/b.rs does not exist",
      "here: no mutant nope.patch",
      "trace dax-cow/NoSuchModel is not recorded: run pnpm traces dax-cow/NoSuchModel",
    ]);
  });

  test("an invariant must cite what it claims: checked cites a mutation or mutant", () => {
    const repo = mkdtempSync(join(tmpdir(), "guide-check-"));
    const content = {
      anchors: {},
      invariants: [
        { id: "x", layer: "guest" as const, claim: "", why: "", enforcedBy: ["missing" as never], evidence: [{ kind: "doc" as const, id: "doc-99" }], standing: "checked" as const },
      ],
      layers: [],
      mutants: {},
      traces: [],
    };
    expect(checkContent(repo, repo, content, none)).toEqual([
      "invariant x: no anchor missing",
      'invariant x: no such doc: {"kind":"doc","id":"doc-99"}',
      'invariant x: is "checked" but cites no mutation or mutant',
    ]);
  });
});
