// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import type { Trace } from "../src/data/types.ts";
import { parseDeadEnd, parseMutant, parseMutation, readEvidence } from "../tools/lib/evidence.ts";
import { GUIDE } from "../tools/lib/repo.ts";

describe("the repo's evidence", () => {
  const evidence = readEvidence();

  test("every mutant names its kernel and killer, and changes a file", () => {
    for (const m of evidence.mutants) {
      expect(["sim", "model", "release", "debug", "kasan"]).toContain(m.kernel);
      expect(m.killer).not.toBe("");
      expect(m.touches.length).toBeGreaterThanOrEqual(1);
    }
  });

  test("a mutant's header parses into its kernel, killer and story", () => {
    const m = parseMutant(
      "x.patch",
      "# SPDX-License-Identifier: GPL-2.0\n# Copyright 2026 The fxdax Authors.\n#\n# Kernel: sim\n# Killer: scenarios cow_swap_during_write\n#\n# TASK-1: a swap\n# under the semaphore shared.\n\n--- a/guest/linux/fs/fxfs/dax.c\n+++ b/guest/linux/fs/fxfs/dax.c\n@@ -1 +1 @@\n-a\n+b\n",
    );
    expect(m).toEqual({
      file: "x.patch",
      kernel: "sim",
      killer: "scenarios cow_swap_during_write",
      story: "TASK-1: a swap under the semaphore shared.",
      touches: ["guest/linux/fs/fxfs/dax.c"],
      patch: "--- a/guest/linux/fs/fxfs/dax.c\n+++ b/guest/linux/fs/fxfs/dax.c\n@@ -1 +1 @@\n-a\n+b\n",
    });
  });

  test("each of dax-cow's mutations removes exactly one rule", () => {
    const daxCow = evidence.mutations.filter((m) => m.spec === "dax-cow");
    expect(daxCow.map((m) => [m.name, m.removed, m.expect])).toEqual([
      ["NoBytesFirst", ["bytesFirst"], "CrashOldOrNew"],
      ["NoCountPrivate", ["countPrivate"], "NoRefusal"],
      ["NoKeepUntilSealed", ["keepUntilSealed"], "NoOpenMap"],
      ["NoPrivateAtIomap", ["privateAtIomap"], "NoLostStore"],
      ["NoProtectByPageTables", ["protectByPageTables"], "CleanInFxfs"],
      ["NoRebindAfterSeal", ["rebindAfterSeal"], "NoOpenMap"],
      ["NoRebindExclusive", ["rebindExclusive"], "NoLostStore"],
      ["NoSwapExclusive", ["swapExclusive"], "FsyncKeepsStores"],
      ["NoSwapFirst", ["swapFirst"], "FsyncKeepsStores"],
    ]);
  });

  test("a mutation's comment, expectation and removed rules parse", () => {
    const m = parseMutation("s", "NoX", '\\* Without x: it breaks.\n\\* expect: Safe\nSPECIFICATION Spec\nCONSTANTS\n    Rules = {"y"}\n', ["x", "y"]);
    expect(m).toEqual({ spec: "s", name: "NoX", story: "Without x: it breaks.", expect: "Safe", removed: ["x"] });
  });

  test("dax-cow's spec gives every rule its description, and its invariants their bodies", () => {
    const spec = evidence.specs.find((s) => s.spec === "dax-cow");
    expect(Object.keys(spec?.rules ?? {}).sort()).toEqual(
      ["bytesFirst", "countPrivate", "keepUntilSealed", "privateAtIomap", "protectByPageTables", "rebindAfterSeal", "rebindExclusive", "swapExclusive", "swapFirst"],
    );
    expect(spec?.invariants.NoLostStore).toBe("NoLostStore == \\A p \\in Pages : win[p].data = latest[p]");
    expect(spec?.definitions.WSwap).toBe("The dirty set swapped out for an empty one; the semaphore downgraded, held shared to the commit.");
  });

  test("a dead end's fields parse", () => {
    const d = parseDeadEnd("theory-x.md", "# Wrong theory: x\n\n**Tried:** today.\n**Idea:** y\nwrapped.\n**Result:** Dropped.\n**Why it's dead:** z.\n");
    expect(d).toEqual({ file: "theory-x.md", title: "Wrong theory: x", theory: true, tried: "today.", idea: "y wrapped.", result: "Dropped.", why: "z." });
  });

  test("docs carry their titles unquoted", () => {
    expect(evidence.docs.find((d) => d.id === "doc-3")?.title).toBe("TASK-26.1 spike: porting Fxfs into the guest's Linux kernel");
  });

  test("each recorded trace violates its mutation's expected invariant, in its last state", () => {
    const dir = join(GUIDE, "fixtures", "traces", "dax-cow");
    for (const file of readdirSync(dir)) {
      const trace = JSON.parse(readFileSync(join(dir, file), "utf8")) as Trace;
      const mutation = evidence.mutations.find((m) => m.spec === "dax-cow" && m.name === trace.mutation);
      expect(trace.violated).toBe(mutation?.expect);
      expect(trace.states[0].action).toBe("Init");
      expect(Object.keys(trace.states[0].values).sort()).toEqual([...trace.vars].sort());
    }
  });
});
