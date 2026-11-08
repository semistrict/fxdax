// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// What this reader has done, in this browser: predictions, designs, knowledge marks, transfer
// times, and the notebook, every interaction and comment in order, which the reader copies out
// for Claude. Browser storage may be missing or throw (private windows, blocked storage); the guide
// works without it, as if nothing was kept.

import { useCallback, useSyncExternalStore } from "react";

export type Mark = "know" | "fuzzy" | "unknown";

/** Where in the guide something happened: the view, and what the reader was looking at. */
export interface Place {
  /** The view's name and step, as the reader sees it: "Page Pilot, step 2: A store in fork A". */
  view: string;
  /** The hash of the view, to find it again. */
  hash: string;
  /** Text the reader selected or the item commented on, quoted. */
  quote?: string;
  /** The code the place is on, if any. */
  code?: { path: string; snippet: string };
}

/** One thing the reader did or said, in the notebook. */
export type Entry = { at: number; place: Place } & (
  | { kind: "prediction"; question: string; chose: string; right: boolean; answer: string }
  | { kind: "design"; prompt: string; text: string }
  | { kind: "catch"; rule: string; guessed: string; actual: string }
  | { kind: "find"; task: string; picked: string; right: boolean; ms: number }
  | { kind: "mark"; item: string; mark: Mark | "cleared" }
  | { kind: "question" | "note"; text: string }
);

export interface Progress {
  /** Everything the reader did and said, in order: what the notebook copies out. */
  notebook: Entry[];
  /** How many of the notebook's entries were copied out already. */
  copied: number;
  /** The option index chosen for each prediction, by mission and prediction id. */
  answers: Record<string, number>;
  /** The reader's own design, by id, written before the reveal. */
  designs: Record<string, string>;
  /** What the reader says they know, by layer or invariant id. */
  marks: Record<string, Mark>;
  /** Transfer tasks: when started and found, ms since the epoch, by task id. */
  transfer: Record<string, { started: number; found?: number; picked?: string }>;
  /** The rule removed, and the invariant guessed, in the safety-catch lab. */
  catches: Record<string, string>;
}

const KEY = "fxdax-flight-school/v1";
const empty = (): Progress => ({ notebook: [], copied: 0, answers: {}, designs: {}, marks: {}, transfer: {}, catches: {} });

function read(): Progress {
  try {
    const text = localStorage.getItem(KEY);
    return text ? { ...empty(), ...(JSON.parse(text) as Partial<Progress>) } : empty();
  } catch {
    return empty();
  }
}

let state: Progress = read();
const listeners = new Set<() => void>();

function write(next: Progress) {
  state = next;
  try {
    localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    // Kept for this page's life only.
  }
  for (const listener of listeners) listener();
}

export function update(change: (draft: Progress) => Progress) {
  write(change(state));
}

export function useProgress(): [Progress, (change: (draft: Progress) => Progress) => void] {
  const snapshot = useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => state,
  );
  return [snapshot, useCallback(update, [])];
}

type WithoutAt<T> = T extends unknown ? Omit<T, "at"> : never;

/** Adds `entry` to the notebook, stamped now. */
export function record(entry: WithoutAt<Entry>) {
  update((p) => ({ ...p, notebook: [...p.notebook, { ...entry, at: Date.now() } as Entry] }));
}

export function forget(at: number) {
  update((p) => {
    const index = p.notebook.findIndex((e) => e.at === at);
    if (index < 0) return p;
    return { ...p, notebook: p.notebook.filter((_, i) => i !== index), copied: index < p.copied ? p.copied - 1 : p.copied };
  });
}

export function exportProgress(): string {
  return JSON.stringify(state, null, 2);
}

export function importProgress(text: string) {
  write({ ...empty(), ...(JSON.parse(text) as Partial<Progress>) });
}

export function resetProgress() {
  write(empty());
}
