// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Page Pilot's machine: one page of one file, in two forks of one base, and the host's memory
// behind their windows. Each scene is a moment of doc-13's protocol; the diagram draws it.

/** What a window page maps (doc-13's invariant 2): a sealed extent's page, zeros, or private. */
export interface WindowPage {
  kind: "extent" | "zeros" | "private";
  /** The extent whose page it maps, for "extent". */
  extent?: string;
  /** What the page holds: a version of the file's page. */
  data: string;
}

export interface Fork {
  name: string;
  page: WindowPage;
  /** The guest's PTE for the user's mapping of the page. */
  pte: "none" | "ro" | "rw";
  /**
   * The guest's sets the page is in: the file's dirty set; writeback's set, from its swap to its
   * commit; and the set kept until the seal after that commit (doc-13's keepUntilSealed).
   */
  sets: ("dirty" | "writeback" | "sealing")[];
}

export interface Extent {
  id: string;
  /** Whose branch minted it. */
  branch: string;
  state: "open" | "sealed";
  data: string;
  /** Whether its page is in the host's extent cache. */
  cached: boolean;
}

export interface Scene {
  a: Fork;
  b: Fork;
  extents: Extent[];
  /** Who holds fork A's file semaphore, and how. */
  semaphore: "none" | "shared" | "exclusive";
  /** The request on the wire, if one is in flight. */
  request?: string;
  /** What changed, in a line, shown with the diagram. */
  caption: string;
  /** The parts of the diagram this moment is about. */
  focus: ("a" | "b" | "cache" | "private" | "extents" | "semaphore")[];
}

const base: Extent = { id: "base·7", branch: "base", state: "sealed", data: "v1", cached: true };

const shared = (name: string): Fork => ({
  name,
  page: { kind: "extent", extent: "base·7", data: "v1" },
  pte: "ro",
  sets: [],
});

export const sharedScene: Scene = {
  a: shared("Fork A"),
  b: shared("Fork B"),
  extents: [base],
  semaphore: "none",
  caption: "Both windows map page 0 of base·7, a sealed extent of the base both forks inherited.",
  focus: ["a", "b", "cache"],
};

export const storeScenes: Scene[] = [
  {
    ...sharedScene,
    semaphore: "shared",
    request: "PRIVATE",
    caption:
      "A's store faults. iomap_begin, the file's semaphore held shared, asks the host for a private copy first.",
    focus: ["a", "semaphore"],
  },
  {
    ...sharedScene,
    a: { ...shared("Fork A"), page: { kind: "private", data: "v1" }, sets: ["dirty"] },
    semaphore: "shared",
    caption:
      "The host copies the page into A's private memory at the same window offset. Then the page joins the dirty set.",
    focus: ["a", "private"],
  },
  {
    ...sharedScene,
    a: { name: "Fork A", page: { kind: "private", data: "v2" }, pte: "rw", sets: ["dirty"] },
    semaphore: "none",
    caption: "Mapped writable, A's store lands in its private page. B still maps base·7: v1.",
    focus: ["a", "b", "private"],
  },
];

const dirtyA: Fork = { name: "Fork A", page: { kind: "private", data: "v2" }, pte: "rw", sets: ["dirty"] };
const afterStore: Scene = { ...sharedScene, a: dirtyA, focus: ["a"], caption: "" };

export const fsyncScenes: Scene[] = [
  {
    ...afterStore,
    a: { ...dirtyA, sets: ["writeback"] },
    semaphore: "exclusive",
    caption:
      "fsync's writeback takes the semaphore exclusively and swaps the dirty set out for an empty one.",
    focus: ["a", "semaphore"],
  },
  {
    ...afterStore,
    a: { ...dirtyA, pte: "ro", sets: ["writeback"] },
    semaphore: "shared",
    caption:
      "It write-protects the page's PTEs, by the page tables: a later store faults into the new dirty set.",
    focus: ["a"],
  },
  {
    ...afterStore,
    a: { ...dirtyA, pte: "ro", sets: ["writeback"] },
    extents: [base, { id: "A·12", branch: "A", state: "open", data: "v2", cached: false }],
    semaphore: "shared",
    request: "WRITE",
    caption:
      "It copies the page out of the window and WRITEs it to a new extent of A's branch, A·12, open.",
    focus: ["extents"],
  },
  {
    ...afterStore,
    a: { ...dirtyA, pte: "ro", sets: ["sealing"] },
    extents: [base, { id: "A·12", branch: "A", state: "open", data: "v2", cached: false }],
    semaphore: "shared",
    caption:
      "Only after its bytes, it commits the transaction naming A·12 as page 0's block, and keeps the page until A·12 is sealed.",
    focus: ["extents"],
  },
  {
    ...afterStore,
    a: { ...dirtyA, pte: "ro", sets: [] },
    extents: [base, { id: "A·12", branch: "A", state: "sealed", data: "v2", cached: false }],
    semaphore: "none",
    request: "COMMIT",
    caption:
      "The seal: COMMIT seals A·12. Nothing in either window changes. fsync returns: ordered, durable on disk within ~50 ms.",
    focus: ["extents", "a", "b"],
  },
  {
    ...afterStore,
    a: { name: "Fork A", page: { kind: "extent", extent: "A·12", data: "v2" }, pte: "ro", sets: [] },
    extents: [base, { id: "A·12", branch: "A", state: "sealed", data: "v2", cached: true }],
    semaphore: "exclusive",
    request: "MAP",
    caption:
      "Later, a rebind, the semaphore exclusive: the page is private, clean, its extent sealed. MAP A·12 over it; the host frees the private copy.",
    focus: ["a", "cache", "semaphore"],
  },
];
