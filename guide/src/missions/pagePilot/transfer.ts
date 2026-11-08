// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { anchors } from "../../content/anchors.ts";
import type { Sym } from "../../data/load.ts";

// The diagram-free check at the end of Page Pilot: from the idea to the code, timed.

export interface TransferTask {
  id: string;
  question: string;
  accepts: (sym: Sym) => boolean;
  /** Said after a wrong pick. */
  hint: string;
  answer: { path: string; snippet: string; note: string };
}

const answer = (id: keyof typeof anchors) => anchors[id];

export const transferTasks: TransferTask[] = [
  {
    id: "private-before-dirty",
    question: "Where does the guest have the host make a page private, before the page goes in the dirty set?",
    accepts: (s) =>
      (s.path === "guest/linux/fs/fxfs/dax.c" && s.name === "fxfs_dax_dirty") ||
      (s.path === "guest/linux/fs/fxfs/device.c" && s.name === "fxfs_device_private"),
    hint: "It is in the guest's C, on the path an iomap_begin takes for a write.",
    answer: answer("guest_dirty"),
  },
  {
    id: "host-copy",
    question: "Where does the host copy a page into the VM's private memory when the guest asks for it?",
    accepts: (s) => (s.path === "crates/fxdax/src/dax.rs" || s.path === "crates/fxdax/src/dax/uffd.rs") && s.name === "make_private",
    hint: "It is the daemon's window code, in crates/fxdax: what serves PRIVATE.",
    answer: answer("window_make_private"),
  },
  {
    id: "rebind",
    question: "Where does the guest map its clean private pages back to their sealed extents?",
    accepts: (s) => s.path === "guest/linux/fs/fxfs/dax.c" && (s.name === "fxfs_dax_rebind_range" || s.name === "fxfs_dax_rebind"),
    hint: "The guest's dax.c calls it a rebind.",
    answer: answer("guest_rebind_range"),
  },
];
