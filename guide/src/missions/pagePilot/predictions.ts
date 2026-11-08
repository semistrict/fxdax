// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Page Pilot's questions, asked before each reveal. Wrong options name the misconception they
// stand for, and the reply says why it fails, with the evidence the reveal then shows.

export interface Option {
  text: string;
  right: boolean;
  /** Shown once chosen: why it holds, or what it misses. */
  why: string;
}

export interface Prediction {
  question: string;
  options: Option[];
}

export const predictions = {
  copies: {
    question: "How many copies of page 0 are in the host's memory now?",
    options: [
      {
        text: "One",
        right: true,
        why:
          "The cache keys sealed extents by ID. Both forks inherited base·7 from the base, so both windows map one physical page, read-only.",
      },
      {
        text: "Two: one behind each VM's window",
        right: false,
        why:
          "A window is guest physical address space, not memory. The pages behind it are the daemon's, shared by every window that maps them.",
      },
      {
        text: "Three: the host's, and one in each guest's page cache",
        right: false,
        why:
          "A DAX file has no guest page cache: loads hit the window. doc-13 measured 22 MB of guest page cache per VM with DAX, against 469 MB without.",
      },
    ],
  },
  forkB: {
    question: "Fork A stores one byte to page 0. Fork B then reads page 0. What does B see?",
    options: [
      { text: "The old byte", right: true, why: "B still maps base·7, which never changes: it is sealed. A's store went to a page of its own." },
      { text: "A's new byte", right: false, why: "Forks share nothing writable. A sealed extent never changes, so no store can reach B through it." },
      { text: "It faults, and waits for A's fsync", right: false, why: "Nothing about B's mapping changes. B never hears of A's store." },
    ],
  },
  address: {
    question: "Does page 0's guest physical address in fork A change when A stores to it?",
    options: [
      {
        text: "No: same window offset, now backed by a private page",
        right: true,
        why: "doc-13's invariant 2: a private page's window address never changes. No store moves a page, so no guest PTE changes for a store.",
      },
      {
        text: "Yes: the guest remaps it to a fresh page",
        right: false,
        why: "That is what writable DAX had to do at every seal, and why it was deleted: a page moving under live PTEs, racing the next seal.",
      },
    ],
  },
  copier: {
    question: "Who copies the page, and when?",
    options: [
      {
        text: "The host, when the guest's iomap_begin asks (PRIVATE), before the page goes in the dirty set",
        right: true,
        why: "Dirty implies private. The guest asks first, so the host never copies at a store.",
      },
      {
        text: "The host, when the store itself faults on the read-only page",
        right: false,
        why:
          "The design before TASK-164.9. A fault's store lands after its iomap_end, maybe after a writeback moved the block and a seal freed it: the host would copy the freed block's zeros. guest/sim found it (seed 2802).",
      },
      {
        text: "The guest, into its page cache",
        right: false,
        why: "One address_space can't hold both DAX entries and folios (doc-13, \"Linux facts\"). The copy is the host's, behind the window.",
      },
    ],
  },
  sealChanges: {
    question: "The COMMIT seals A·12. Does it change anything fork A's window maps?",
    options: [
      {
        text: "No",
        right: true,
        why: "The window maps only sealed extents and zeros, and private pages no extent names. A seal changes none of them.",
      },
      {
        text: "Yes: page 0 moves to A·12 at the seal",
        right: false,
        why: "Moving pages at seals is writable DAX's design, which doc-13 replaced. The page goes back to an extent later, by a rebind, which the guest asks for.",
      },
    ],
  },
  oldStore: {
    question:
      "Before copy-on-write, a store went straight to the open extent's block, mapped writable in the window. Every fsync seals every open extent. What must the next store to that page do?",
    options: [
      { text: "Nothing: the block is still the page's", right: false, why: "Sealed means immutable. After any fsync, anywhere in the filesystem, the page's block can't take a store." },
      {
        text: "Move the page to a new open block, in the fault path, while the next seal may come",
        right: true,
        why: "Under every PTE and reader that maps it. doc-13 counts what that cost: about 25k lines, 7 kernel patches, 37–39 of 65 dead ends, and DAX at 0.51–0.61× no-DAX under pgbench.",
      },
      { text: "Wait for the upload to the bucket", right: false, why: "Uploads trail commits by design. Only sync, syncfs, unmount and freeze wait for one (doc-12)." },
    ],
  },
} satisfies Record<string, Prediction>;

export type PredictionId = keyof typeof predictions;
