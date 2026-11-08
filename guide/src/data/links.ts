// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Code is found by text, never by line (lines drift with every edit), in the local checkout: the
// guide shows it from the copy its build made (tools/sources.ts), and gives the `rg` that finds
// it in a checkout.

/** `text` quoted for a POSIX shell. */
export function shellQuote(text: string): string {
  return `'${text.replaceAll("'", `'\\''`)}'`;
}

/** The command that finds `snippet` in `path`, from the repo's root. */
export function rgCommand(path: string, snippet: string): string {
  return `rg -n -F ${shellQuote(snippet)} ${path}`;
}

/** Where the guide's copy of a repo file is served from, relative to the page. */
export function sourceUrl(path: string): string {
  return `source/${path.split("/").map(encodeURIComponent).join("/")}`;
}

/**
 * Every file of the guide's copy, by path, in the single-file build, where nothing is beside the
 * page to fetch (tools/sources.ts writes it). Other builds never load it: the mode is known at
 * build time, and the import goes with the branch.
 */
let inline: Promise<Record<string, string>> | null = null;
function inlineSources(): Promise<Record<string, string>> {
  inline ??= import("../generated/sources-inline.json?raw").then((m) => JSON.parse(m.default) as Record<string, string>);
  return inline;
}

/** A repo file's text, from the guide's copy, fetched once. */
const cache = new Map<string, Promise<string>>();
export function fetchSource(path: string): Promise<string> {
  let text = cache.get(path);
  if (!text) {
    text =
      import.meta.env.MODE === "single"
        ? inlineSources().then((all) => {
            const found = all[path];
            if (found === undefined) throw new Error(`${path}: not in the guide`);
            return found;
          })
        : fetch(sourceUrl(path)).then((r) => {
            if (!r.ok) throw new Error(`${path}: ${r.status}`);
            return r.text();
          });
    cache.set(path, text);
  }
  return text;
}

/**
 * The lines to show around `snippet` in `text`: the comments right above it, then it to the end of
 * the block it opens (by brace depth from its line), at most `max` lines. A snippet that is a
 * statement (its line ends with `;` before any `{`) gets `around` lines either side instead.
 * 0-based [first, last], and the snippet's first line; null if the snippet isn't in the text.
 */
export function excerpt(text: string, snippet: string, max = 80, around = 10): { first: number; last: number; at: number } | null {
  const offset = text.indexOf(snippet);
  if (offset < 0) return null;
  const lines = text.split("\n");
  const at = text.slice(0, offset).split("\n").length - 1;
  if (/;\s*$/.test(lines[at]) && !lines[at].includes("{")) {
    return { first: Math.max(0, at - around), last: Math.min(lines.length - 1, at + around), at };
  }
  let first = at;
  while (first > 0 && first > at - 24 && /^\s*(\/\/|\/\*|\*|#\[|\\\*)/.test(lines[first - 1])) first--;
  let depth = 0;
  let opened = false;
  let last = at;
  for (let i = at; i < lines.length && i < at + max; i++) {
    for (const ch of lines[i]) {
      if (ch === "{") {
        depth++;
        opened = true;
      } else if (ch === "}") depth--;
    }
    last = i;
    if (opened && depth <= 0) break;
    // A declaration that opens no block: a bodiless fn, a unit struct, a C prototype.
    if (!opened && /;\s*$/.test(lines[i])) break;
  }
  return { first, last, at };
}
