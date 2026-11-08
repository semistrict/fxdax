// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import { crateOrigins, fileOrigin, originOf, REPO, standIns } from "../tools/lib/repo.ts";
import { firstSentence, indexRepo, isTestPath } from "../tools/lib/symbols.ts";

describe("where code comes from", () => {
  const crates = crateOrigins();

  test("the stand-ins are README's table's, and zx-sys", () => {
    expect([...standIns()].sort()).toEqual([
      "bssl-crypto",
      "diagnostics-log",
      "fidl",
      "fidl-fuchsia-diagnostics-types",
      "fidl-fuchsia-fs",
      "fidl-fuchsia-fxfs",
      "fidl-fuchsia-io",
      "fuchsia-inspect",
      "fxfs-unicode-gen",
      "mundane",
      "zx-sys",
    ]);
  });

  test.each([
    ["crates/fxfs/src/object_store/journal.rs", "fuchsia-changed"],
    ["crates/fxfs/src/object_store/volume/snapshot.rs", "fxdax"],
    ["crates/storage_device/src/extent.rs", "fxdax"],
    ["crates/zx/src/lib.rs", "fuchsia"],
    ["crates/fidl/src/lib.rs", "stand-in"],
    ["crates/fxdax/src/extents.rs", "fxdax"],
    ["crates/extent_remote/src/writer.rs", "fxdax"],
    ["guest/linux/fs/fxfs/dax.c", "fxdax"],
  ])("%s is %s", (path, origin) => {
    expect(originOf(path, crates)).toBe(origin);
  });

  test("a crate copied from Fuchsia cites its Fuchsia directory; fxdax's don't", () => {
    expect(crates.get("fxfs")).toBe("fuchsia");
    expect(crates.get("fuchsia-async")).toBe("fuchsia");
    expect(crates.get("fxdax")).toBe("fxdax");
    expect(crates.get("physmem")).toBe("fxdax");
  });

  test("in fxdax's own crates, a file citing a Fuchsia source is ported", () => {
    expect(fileOrigin("fxdax", "// Ported from `zircon/kernel/vm/page_queues.cc`.\nfn x() {}\n")).toBe("ported");
    expect(fileOrigin("fxdax", "// fxdax's own.\nfn x() {}\n")).toBe("fxdax");
  });
});

describe("the symbol index", () => {
  test("first sentences end at a sentence's full stop, not an abbreviation's or a path's", () => {
    expect(firstSentence("Reads `a.b`, e.g. the root. Then more.")).toBe("Reads `a.b`, e.g. the root.");
    expect(firstSentence("Maps it: read-only.\nNext.")).toBe("Maps it: read-only.");
  });

  test("test code is left out", () => {
    expect(isTestPath("crates/fxdax/src/extentd/tests/open.rs")).toBe(true);
    expect(isTestPath("crates/fxdax/src/cache_tests.rs")).toBe(true);
    expect(isTestPath("crates/fxdax/src/cache.rs")).toBe(false);
  });

  test("it finds fxdax's and Fuchsia's symbols, with their docs and anchors", async () => {
    const symbols = await indexRepo();
    const store = symbols.find((s) => s.path === "crates/fxdax/src/extents.rs" && s.name === "ExtentStore" && s.kind === "struct");
    expect(store).toEqual({
      path: "crates/fxdax/src/extents.rs",
      kind: "struct",
      name: "ExtentStore",
      container: "",
      signature: "pub struct ExtentStore",
      unique: true,
      doc: "An extent store a guest's filesystem lives on.",
      origin: "fxdax",
    });
    const commit = symbols.find((s) => s.path === "crates/fxfs/src/object_store/journal.rs" && s.name === "commit" && s.container === "Journal");
    expect(commit?.signature).toBe("pub async fn commit(&self, transaction: &mut Transaction<'_>) -> Result<u64, Error>");
    expect(commit?.origin).toBe("fuchsia-changed");
    const dirty = symbols.find((s) => s.path === "guest/linux/fs/fxfs/dax.c" && s.name === "fxfs_dax_dirty");
    expect(dirty?.kind).toBe("fn");
    for (const sym of symbols.filter((s) => s.unique).slice(0, 500)) {
      const text = readFileSync(join(REPO, sym.path), "utf8");
      expect(text.indexOf(sym.signature)).toBe(text.lastIndexOf(sym.signature));
    }
  });
});
