// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The shapes the guide's tools write and its pages read.

export type Origin = "fxdax" | "ported" | "fuchsia" | "fuchsia-changed" | "stand-in";

export type Kind = "fn" | "struct" | "enum" | "trait" | "mod" | "const" | "static" | "type";

/** src/generated/symbols.json. */
export interface SymbolIndex {
  /** The repo's commit, and the vendored Firecracker's, the index was made at. */
  commit: string;
  firecracker: string;
  /** [path, origin] of each file indexed. */
  files: [string, Origin][];
  /** [file, kind, name, container, signature, doc, unique (1) or not (0)]. */
  symbols: [number, Kind, string, string, string, string, 0 | 1][];
}

/** A past bug of the guest's put back (guest/mutants/*.patch). */
export interface Mutant {
  file: string;
  kernel: string;
  /** The run that must fail with the mutant in, or "none". */
  killer: string;
  /** What the bug was, how it was found, how its killer fails: the header's other comments. */
  story: string;
  /** The files the patch changes. */
  touches: string[];
  /** The patch itself, which takes the fix away. */
  patch: string;
}

/** A TLA+ model with one rule of its spec removed (specs/SPEC/mutations/NAME.cfg). */
export interface Mutation {
  spec: string;
  name: string;
  /** What breaks, as the configuration's comment says. */
  story: string;
  /** The invariant TLC must find violated, or `property NAME`, or `deadlock`. */
  expect: string;
  /** The rules of the spec's AllRules this model leaves out of Rules. */
  removed: string[];
}

/** A spec's own words: its header, rules, actions and invariants (specs/SPEC/SPEC.tla). */
export interface Spec {
  spec: string;
  path: string;
  /** The header comment's text, before the rules. */
  summary: string;
  /** Each rule a mutation may remove, with the header's description of it. */
  rules: Record<string, string>;
  /** Each top-level definition's preceding comment, by name. */
  definitions: Record<string, string>;
  /** Each invariant's body, by name, for those its models check. */
  invariants: Record<string, string>;
}

/** An approach tried and abandoned (dead-ends/*.md). */
export interface DeadEnd {
  file: string;
  title: string;
  /** Whether it is a theory that turned out wrong, rather than an approach. */
  theory: boolean;
  tried: string;
  idea: string;
  result: string;
  why: string;
}

/** A backlog design doc (backlog/docs/doc-N - Title.md). */
export interface Doc {
  id: string;
  title: string;
  file: string;
  headings: string[];
}

/** src/generated/evidence.json. */
export interface Evidence {
  mutants: Mutant[];
  mutations: Mutation[];
  specs: Spec[];
  deadEnds: DeadEnd[];
  docs: Doc[];
}

/** A TLC counterexample (fixtures/traces/SPEC/NAME.json), as `pnpm traces` recorded it. */
export interface Trace {
  spec: string;
  mutation: string;
  /** The invariant violated in the last state, as TLC said. */
  violated: string;
  /** The TLC release and the repo commit the trace was recorded with. */
  tlc: string;
  commit: string;
  /** The variables, in TLC's order. */
  vars: string[];
  /** Each state: the action that led to it ("Init" for the first), and its values. */
  states: { action: string; values: Record<string, unknown> }[];
}
