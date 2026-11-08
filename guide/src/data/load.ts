// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The generated data the pages read: the evidence, bundled; the symbol index, loaded when first
// needed (it is most of the guide's weight); the TLC traces, each its own chunk.

import { useEffect, useState } from "react";
import evidenceJson from "../generated/evidence.json";
import metaJson from "../generated/meta.json";
import type { Evidence, Kind, Origin, SymbolIndex, Trace } from "./types.ts";

export const evidence = evidenceJson as unknown as Evidence;

/** The commits the guide was built from: the repo's, and the vendored Firecracker's. */
export const meta = metaJson as { commit: string; firecracker: string };

export interface Sym {
  path: string;
  origin: Origin;
  kind: Kind;
  name: string;
  container: string;
  signature: string;
  doc: string;
  unique: boolean;
}

export interface LoadedIndex {
  commit: string;
  firecracker: string;
  symbols: Sym[];
  files: { path: string; origin: Origin }[];
  /** Each file's symbols by signature, to find the symbol an anchor's snippet begins. */
  bySignature: Map<string, Sym>;
}

let indexPromise: Promise<LoadedIndex> | null = null;

export function loadIndex(): Promise<LoadedIndex> {
  indexPromise ??= import("../generated/symbols.json").then((module) => {
    const raw = module.default as unknown as SymbolIndex;
    const files = raw.files.map(([path, origin]) => ({ path, origin }));
    const symbols = raw.symbols.map(([file, kind, name, container, signature, doc, unique]): Sym => ({
      path: files[file].path,
      origin: files[file].origin,
      kind,
      name,
      container,
      signature,
      doc,
      unique: unique === 1,
    }));
    const bySignature = new Map<string, Sym>();
    for (const sym of symbols) bySignature.set(`${sym.path}\n${sym.signature}`, sym);
    return { commit: raw.commit, firecracker: raw.firecracker, symbols, files, bySignature };
  });
  return indexPromise;
}

export function useIndex(): LoadedIndex | null {
  const [index, setIndex] = useState<LoadedIndex | null>(null);
  useEffect(() => {
    let live = true;
    void loadIndex().then((loaded) => live && setIndex(loaded));
    return () => {
      live = false;
    };
  }, []);
  return index;
}

const traceModules = import.meta.glob<Trace>("../../fixtures/traces/*/*.json", { import: "default" });

export function loadTrace(spec: string, mutation: string): Promise<Trace> | null {
  const load = traceModules[`../../fixtures/traces/${spec}/${mutation}.json`];
  return load ? load() : null;
}

export function hasTrace(spec: string, mutation: string): boolean {
  return `../../fixtures/traces/${spec}/${mutation}.json` in traceModules;
}
