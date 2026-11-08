// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import type { Place } from "./progress.ts";

// Where the reader is, which every notebook entry records: the view names itself as it renders.

let current: { view: string; hash: string } = { view: "Atlas", hash: "atlas" };

export function setHere(view: string, hash: string) {
  current = { view, hash };
}

export function here(extra: Omit<Place, "view" | "hash"> = {}): Place {
  return { ...current, ...extra };
}

/** The code an element is on, from the nearest code card's data attributes. */
export function codeAt(element: Element | null): Place["code"] {
  const card = element?.closest<HTMLElement>("[data-code-path]");
  const path = card?.dataset.codePath;
  const snippet = card?.dataset.codeSnippet;
  return path && snippet ? { path, snippet } : undefined;
}
