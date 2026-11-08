// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { describe, expect, test } from "vitest";
import { anchors } from "../src/content/anchors.ts";
import { excerpt, rgCommand, shellQuote, sourceUrl } from "../src/data/links.ts";
import { learningPrompt } from "../src/atlas/prompt.ts";
import { predictions } from "../src/missions/pagePilot/predictions.ts";
import { fsyncScenes, sharedScene, storeScenes } from "../src/missions/pagePilot/scene.ts";
import { transferTasks } from "../src/missions/pagePilot/transfer.ts";
import { indexRepo } from "../tools/lib/symbols.ts";

describe("code by text, from the local source", () => {
  test("rg gets the snippet quoted for a shell, quotes and all", () => {
    expect(shellQuote("Transaction<'_>")).toBe("'Transaction<'\\''_>'");
    expect(rgCommand("crates/fxdax/src/extents.rs", "pub struct ExtentStore")).toBe(
      "rg -n -F 'pub struct ExtentStore' crates/fxdax/src/extents.rs",
    );
  });

  test("the guide's copy of a file is served beside the page, its path escaped", () => {
    expect(sourceUrl("backlog/docs/doc-13 - Copy-on-write-DAX.md")).toBe("source/backlog/docs/doc-13%20-%20Copy-on-write-DAX.md");
  });

  test("an excerpt runs from the comments above the snippet to the end of the block it opens", () => {
    const text = ["use x;", "", "/// Does it.", "/// Twice.", "fn f(", "    a: u8,", ") {", "    if a { b(); }", "}", "fn g() {}"].join("\n");
    expect(excerpt(text, "fn f(")).toEqual({ first: 2, last: 8, at: 4 });
  });

  test("a statement's excerpt is the lines around it", () => {
    const text = Array.from({ length: 30 }, (_, i) => (i === 15 ? "    let x = y.z()?;" : `    line(${i});`)).join("\n");
    expect(excerpt(text, "let x = y.z()?;")).toEqual({ first: 5, last: 25, at: 15 });
  });

  test("a declaration that opens no block ends at its semicolon", () => {
    expect(excerpt("// A sig.\nfn f(\n    a: u8,\n);\nfn g() {}", "fn f(")).toEqual({ first: 0, last: 3, at: 1 });
  });

  test("a snippet no longer in the file has no excerpt", () => {
    expect(excerpt("fn f() {}", "fn g()")).toBeNull();
  });
});

describe("Page Pilot", () => {
  test("every prediction has exactly one right answer", () => {
    for (const [id, p] of Object.entries(predictions)) {
      expect([id, p.options.filter((o) => o.right).length]).toEqual([id, 1]);
    }
  });

  test("every scene maps only extents it has, and only cached ones", () => {
    for (const scene of [sharedScene, ...storeScenes, ...fsyncScenes]) {
      for (const fork of [scene.a, scene.b]) {
        if (fork.page.kind === "extent") {
          expect(scene.extents.find((e) => e.id === fork.page.extent)).toMatchObject({ state: "sealed", cached: true });
        }
      }
    }
  });

  test("dirty implies private in every scene", () => {
    for (const scene of [sharedScene, ...storeScenes, ...fsyncScenes]) {
      for (const fork of [scene.a, scene.b]) {
        if (fork.sets.length) expect(fork.page.kind).toBe("private");
      }
    }
  });

  test("each transfer task accepts symbols the index has, in its answer's file among them", async () => {
    const symbols = await indexRepo();
    const accepted = transferTasks.map((task) => [task.id, [...new Set(symbols.filter(task.accepts).map((s) => `${s.path} ${s.name}`))].sort()]);
    expect(accepted).toEqual([
      ["private-before-dirty", ["guest/linux/fs/fxfs/dax.c fxfs_dax_dirty", "guest/linux/fs/fxfs/device.c fxfs_device_private"]],
      ["host-copy", ["crates/fxdax/src/dax.rs make_private", "crates/fxdax/src/dax/uffd.rs make_private"]],
      ["rebind", ["guest/linux/fs/fxfs/dax.c fxfs_dax_rebind", "guest/linux/fs/fxfs/dax.c fxfs_dax_rebind_range"]],
    ]);
    for (const task of transferTasks) {
      expect(symbols.some((s) => task.accepts(s) && s.path === task.answer.path)).toBe(true);
    }
  });
});

describe("the learning prompt", () => {
  test("none until something is marked fuzzy or unknown", () => {
    expect(learningPrompt({})).toBeNull();
    expect(learningPrompt({ "layer:guest": "know" })).toBeNull();
  });

  test("it names the gaps with their anchors, and builds on what is known", () => {
    const prompt = learningPrompt({ "layer:memory": "unknown", "inv:fsync-ordered": "fuzzy", "layer:guest": "know" });
    expect(prompt).toContain("I don't understand these yet:\n- The Physical memory layer (the daemon's pages: arenas of shared memory).");
    expect(prompt).toContain(`    - ${anchors.extent_cache.path}: \`${anchors.extent_cache.snippet}\``);
    expect(prompt).toContain("I'm fuzzy on these:\n- The claim \"An fsync returns once its commit is ordered");
    expect(prompt).toContain("I already understand the Guest kernel layer. Build on that.");
    expect(prompt).toContain("never by line number");
  });
});
