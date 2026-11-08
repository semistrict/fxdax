// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import type { AnchorId } from "./anchors.ts";

/** The stack, top to bottom: the atlas's rows, and the colour each view gives the layer. */
export type LayerId = "guest" | "driver" | "vmm" | "daemon" | "memory" | "disk" | "bucket";

export interface Layer {
  id: LayerId;
  name: string;
  /** What it is, in a line. */
  what: string;
  /** What it does in a paragraph, for someone who knows systems but not this one. */
  role: string;
  dirs: string[];
  anchors: AnchorId[];
  docs: string[];
  /** Where its code comes from, in words: whose, and how much is Fuchsia's. */
  provenance: string;
}

/** What a claim of the guide stands on, each a thing the build checks exists. */
export type EvidenceRef =
  | { kind: "mutation"; spec: string; name: string }
  | { kind: "mutant"; file: string }
  | { kind: "deadEnd"; file: string }
  | { kind: "doc"; id: string; heading?: string };

/**
 * How a claim is known:
 * - `checked`: a model checker or a test fails without it (a TLA+ mutation, a mutant's killer);
 * - `stated`: the design docs and the code say it, and no check here shows its violation fails.
 */
export type Standing = "checked" | "stated";

export interface Invariant {
  id: string;
  layer: LayerId;
  /** The claim, which lets the reader predict the code. */
  claim: string;
  /** Why it holds, or what it buys, in a sentence or two. */
  why: string;
  enforcedBy: AnchorId[];
  evidence: EvidenceRef[];
  standing: Standing;
}
