// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { useDeferredValue, useMemo, useState } from "react";
import { type Sym, useIndex } from "../data/load.ts";
import type { Kind, Origin } from "../data/types.ts";
import { CodeRef, ORIGIN_LABEL } from "../ui/code.tsx";

const ORIGINS: Origin[] = ["fxdax", "ported", "fuchsia-changed", "fuchsia", "stand-in"];
const KINDS: Kind[] = ["fn", "struct", "enum", "trait", "mod", "const", "static", "type"];
const SHOWN = 40;

/** How well `sym` matches the query's words: name first, then container, path, doc. 0 is none. */
function score(sym: Sym, words: string[]): number {
  let total = 0;
  const name = sym.name.toLowerCase();
  for (const word of words) {
    if (name === word) total += 100;
    else if (name.startsWith(word)) total += 40;
    else if (name.includes(word)) total += 25;
    else if (sym.container.toLowerCase().includes(word)) total += 12;
    else if (sym.path.toLowerCase().includes(word)) total += 8;
    else if (sym.doc.toLowerCase().includes(word)) total += 5;
    else return 0;
  }
  return total + (sym.doc ? 1 : 0);
}

export function SymbolSearch({
  onPick,
  placeholder = "Search 11,000+ symbols: a name, a path, words of a doc comment",
  autoFocus = false,
}: {
  onPick?: (sym: Sym) => void;
  placeholder?: string;
  autoFocus?: boolean;
}) {
  const index = useIndex();
  const [query, setQuery] = useState("");
  const [origins, setOrigins] = useState<Set<Origin>>(new Set(ORIGINS));
  const [kind, setKind] = useState<Kind | "all">("all");
  const [open, setOpen] = useState<string | null>(null);
  const [more, setMore] = useState(1);
  const deferred = useDeferredValue(query);
  const hits = useMemo(() => {
    if (!index) return [];
    const words = deferred.toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length) return [];
    const found: [number, Sym][] = [];
    for (const sym of index.symbols) {
      if (!origins.has(sym.origin) || (kind !== "all" && sym.kind !== kind)) continue;
      const s = score(sym, words);
      if (s) found.push([s, sym]);
    }
    found.sort((a, b) => b[0] - a[0]);
    return found.map(([, sym]) => sym);
  }, [index, deferred, origins, kind]);
  const counts = useMemo(() => {
    const c = new Map<Origin, number>();
    for (const sym of index?.symbols ?? []) c.set(sym.origin, (c.get(sym.origin) ?? 0) + 1);
    return c;
  }, [index]);

  return (
    <div className="search">
      <input
        id={onPick ? "transfer-search" : "index-search"}
        type="search"
        value={query}
        placeholder={placeholder}
        autoFocus={autoFocus}
        onChange={(e) => {
          setQuery(e.target.value);
          setMore(1);
        }}
        aria-label="Search symbols"
      />
      <div className="row" style={{ justifyContent: "space-between" }}>
        <div className="filters" role="group" aria-label="Where the code comes from">
          {ORIGINS.map((o) => (
            <button
              key={o}
              type="button"
              className={`o-${o}`}
              aria-pressed={origins.has(o)}
              onClick={() => {
                const next = new Set(origins);
                if (next.has(o)) next.delete(o);
                else next.add(o);
                setOrigins(next);
              }}
            >
              {ORIGIN_LABEL[o]} <span className="num">{counts.get(o) ?? ""}</span>
            </button>
          ))}
        </div>
        <select id={onPick ? "transfer-kind" : "index-kind"} value={kind} onChange={(e) => setKind(e.target.value as Kind | "all")} style={{ width: "auto" }} aria-label="Kind">
          <option value="all">every kind</option>
          {KINDS.map((k) => (
            <option key={k} value={k}>
              {k}
            </option>
          ))}
        </select>
      </div>
      {!index && <p className="muted">Loading the index…</p>}
      {index && deferred && (
        <p className="muted num" style={{ fontSize: "0.9rem" }}>
          {hits.length} found{hits.length > SHOWN * more ? `, the first ${SHOWN * more} shown` : ""}
        </p>
      )}
      <div className="hits">
        {hits.slice(0, SHOWN * more).map((sym) => {
          const key = `${sym.path}:${sym.container}:${sym.name}:${sym.signature}`;
          return open === key && !onPick ? (
            <div key={key} style={{ display: "grid", gap: 6 }}>
              <CodeRef
                path={sym.path}
                snippet={sym.signature}
                note={sym.unique ? undefined : "This signature occurs more than once in its file: the code shown is its first."}
                origin={sym.origin}
                open
              />
              <button type="button" className="btn" style={{ justifySelf: "start" }} onClick={() => setOpen(null)}>
                Close {sym.name}
              </button>
            </div>
          ) : (
            <div
              key={key}
              className="hit pickable"
              style={{ "--origin": `var(--o-${sym.origin})` } as React.CSSProperties}
              role="button"
              tabIndex={0}
              onClick={() => (onPick ? onPick(sym) : setOpen(key))}
              onKeyDown={(e) => {
                if (e.key === "Enter") onPick ? onPick(sym) : setOpen(key);
              }}
            >
              <div className="hit-head">
                <span className="hit-name">
                  {sym.container ? <span className="muted">{sym.container}::</span> : null}
                  {sym.name}
                </span>
                <span className="eyebrow">{sym.kind}</span>
                <span className={`chip o-${sym.origin}`}>{ORIGIN_LABEL[sym.origin]}</span>
                <span className="ref-path">{sym.path}</span>
              </div>
              {sym.doc && <div className="hit-doc">{sym.doc}</div>}
            </div>
          );
        })}
      </div>
      {hits.length > SHOWN * more && (
        <button type="button" className="btn" onClick={() => setMore(more + 1)}>
          Show more
        </button>
      )}
    </div>
  );
}
