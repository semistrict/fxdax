// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { describe, expect, test } from "vitest";
import type { Progress } from "../src/data/progress.ts";
import { notebookText } from "../src/notebook/export.ts";

const place = { view: "Page Pilot, step 2: A store in fork A", hash: "pilot-2" };

const progress: Progress = {
  copied: 0,
  answers: {},
  designs: {},
  transfer: {},
  catches: {},
  marks: { "layer:memory": "unknown", "layer:guest": "know" },
  notebook: [
    { at: 1, kind: "prediction", question: "Who copies the page, and when?", chose: "The guest, into its page cache", right: false, answer: "The host, when asked", place },
    { at: 2, kind: "prediction", question: "What does B see?", chose: "The old byte", right: true, answer: "The old byte", place },
    { at: 3, kind: "design", prompt: "Design it before you look.", text: "Write it to a new extent,\nthen remap.", place: { view: "Page Pilot, step 3: fsync", hash: "pilot-3" } },
    { at: 4, kind: "catch", rule: "swapExclusive", guessed: "NoLostStore", actual: "FsyncKeepsStores", place },
    { at: 5, kind: "find", task: "Where does the host copy a page?", picked: "crates/fxdax/src/dax.rs make_private", right: true, ms: 67_000, place },
    {
      at: 6,
      kind: "question",
      text: "Why shared here?",
      place: { ...place, quote: "holding the file's semaphore shared", code: { path: "guest/linux/fs/fxfs/dax.c", snippet: "static int fxfs_dax_dirty(" } },
    },
  ],
};

describe("the notebook's blob for Claude", () => {
  const text = notebookText(progress, 0, "abcdef1234567");

  test("it says who I am and what to do with it", () => {
    expect(text).toContain("from the guide (guide/) built at abcdef123.");
    expect(text).toContain("cite code by exact snippet (path plus text), never by line number.");
  });

  test("it says what I got wrong, with the right answer", () => {
    expect(text).toContain("## Predictions: 1 of 2 right");
    expect(text).toContain(
      '- **Wrong**, on Page Pilot, step 2: A store in fork A: "Who copies the page, and when?" I answered "The guest, into its page cache"; the answer is "The host, when asked".',
    );
    expect(text).toContain("- **Wrong**: without `swapExclusive` I said `NoLostStore` breaks; TLC finds `FsyncKeepsStores` violated first.");
  });

  test("it keeps my designs, finds and marks", () => {
    expect(text).toContain("> Write it to a new extent,\n> then remap.");
    expect(text).toContain('- "Where does the host copy a page?": found `crates/fxdax/src/dax.rs make_private` after 1:07.');
    expect(text).toContain("- I know: the Guest kernel layer.");
    expect(text).toContain("- I don't understand: the Physical memory layer.");
  });

  test("a question carries its quote and the code it was on", () => {
    expect(text).toContain(
      "1. **Question** on Page Pilot, step 2: A store in fork A, at `guest/linux/fs/fxfs/dax.c`: `static int fxfs_dax_dirty(`:\n\n   > holding the file's semaphore shared\n\n   Why shared here?",
    );
  });

  test("from a later entry, only what is new", () => {
    const later = notebookText({ ...progress, copied: 5 }, 5, "abcdef1234567");
    expect(later).toContain("These are my entries since my last export (1 of 6).");
    expect(later).not.toContain("## Predictions");
    expect(later).toContain("Why shared here?");
  });
});
