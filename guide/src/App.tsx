// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { useEffect, useState } from "react";
import { Atlas } from "./atlas/Atlas.tsx";
import { invariants } from "./content/invariants.ts";
import { meta } from "./data/load.ts";
import { DeadEndPage, DocPage } from "./docs/DocPage.tsx";
import { exportProgress, importProgress, resetProgress } from "./data/progress.ts";
import { SymbolSearch } from "./index/Search.tsx";
import { PagePilot } from "./missions/pagePilot/PagePilot.tsx";
import { InvariantCard } from "./ui/evidence.tsx";
import { setHere } from "./data/place.ts";
import { Composer, NotebookButton, NotebookDrawer, SelectionAsk } from "./notebook/Notebook.tsx";

// Views by the hash: #atlas (the start), #pilot or #pilot-N, #invariants or #invariants-ID,
// #index, #progress, #doc-N (a design doc), #dead-NAME (a dead end).

function useHash(): string {
  const [hash, setHash] = useState(() => location.hash.slice(1));
  useEffect(() => {
    const on = () => setHash(location.hash.slice(1));
    addEventListener("hashchange", on);
    return () => removeEventListener("hashchange", on);
  }, []);
  return hash;
}

const NAV = [
  ["atlas", "Atlas"],
  ["pilot", "Page Pilot"],
  ["invariants", "Invariants"],
  ["index", "Index"],
  ["progress", "Progress"],
] as const;

function Invariants({ focus }: { focus: string | null }) {
  useEffect(() => {
    if (focus) document.getElementById(`inv-${focus}`)?.scrollIntoView({ block: "start" });
  }, [focus]);
  return (
    <>
      <div className="lede">
        <span className="eyebrow">Invariants</span>
        <h1>What lets you predict the code</h1>
        <p>
          Each claim is stated as the design docs state it, with the code that keeps it. “Checked” means a TLA+ mutation
          or a past bug put back shows what breaks without it; “stated” means the docs and the code say it, and nothing
          here shows its violation failing.
        </p>
      </div>
      <div className="invs">
        {invariants.map((inv) => (
          <div key={inv.id} id={`inv-${inv.id}`} style={{ scrollMarginTop: 80, display: "grid" }}>
            <InvariantCard inv={inv} />
          </div>
        ))}
      </div>
    </>
  );
}

function ProgressView() {
  const [text, setText] = useState("");
  const [note, setNote] = useState<string | null>(null);
  return (
    <>
      <div className="lede">
        <span className="eyebrow">Progress</span>
        <h1>What this browser remembers</h1>
        <p>Your predictions, designs, marks and times stay in this browser only. Move them by copying them out and in.</p>
      </div>
      <section className="panel" style={{ display: "grid", gap: 10 }}>
        <div className="row">
          <button type="button" className="btn" onClick={() => setText(exportProgress())}>
            Show my progress as JSON
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              try {
                importProgress(text);
                setNote("Loaded.");
              } catch {
                setNote("That isn’t progress JSON: paste what “Show my progress” gave.");
              }
            }}
          >
            Load the JSON below
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              resetProgress();
              setNote("Cleared: every prediction can be made again.");
            }}
          >
            Clear everything
          </button>
        </div>
        {note && <p className="muted">{note}</p>}
        <textarea id="progress-json" value={text} onChange={(e) => setText(e.target.value)} placeholder="Progress JSON" />
      </section>
    </>
  );
}

export function App() {
  const hash = useHash();
  const [view, arg] = (() => {
    const m = /^([a-z]+)(?:-(.+))?$/.exec(hash);
    return m ? [m[1], m[2] ?? null] : ["atlas", null];
  })();
  useEffect(() => {
    if (view !== "invariants" || !arg) scrollTo({ top: 0 });
  }, [view, arg]);
  // Where notebook entries say they were made; the atlas and missions name themselves finer.
  const NAMES: Record<string, string> = { invariants: "Invariants", index: "Index", progress: "Progress" };
  if (NAMES[view]) setHere(NAMES[view], hash || view);
  if (view === "doc" && arg) setHere(`Design doc doc-${arg}`, hash);
  if (view === "dead" && arg) setHere(`Dead end ${arg}`, hash);
  return (
    <>
      <header className="bar">
        <div className="bar-inner">
          <a href="#atlas" className="brand" style={{ textDecoration: "none", color: "inherit" }}>
            fxdax <span>Flight School</span>
          </a>
          <nav className="nav" aria-label="Views">
            {NAV.map(([id, label]) => (
              <a key={id} href={`#${id}`} aria-current={view === id ? "page" : undefined}>
                {label}
              </a>
            ))}
          </nav>
          <NotebookButton />
          <span className="rev" title="The commit this guide was built from">
            built from {meta.commit.slice(0, 9)}
          </span>
        </div>
      </header>
      <main className="page">
        {view === "pilot" ? (
          <PagePilot step={arg ? Number(arg) - 1 : 0} />
        ) : view === "invariants" ? (
          <Invariants focus={arg} />
        ) : view === "index" ? (
          <>
            <div className="lede">
              <span className="eyebrow">Index</span>
              <h1>Every symbol, Fuchsia’s included</h1>
              <p>
                Functions, types, traits, modules, consts and statics of fxdax and of the Fuchsia code it copies, from
                tree-sitter, with each one’s first doc sentence. Open one for its links: a search for its exact signature,
                never a line number.
              </p>
            </div>
            <SymbolSearch autoFocus />
          </>
        ) : view === "progress" ? (
          <ProgressView />
        ) : view === "doc" && arg ? (
          <DocPage id={`doc-${arg}`} />
        ) : view === "dead" && arg ? (
          <DeadEndPage slug={arg} />
        ) : (
          <Atlas />
        )}
      </main>
      <SelectionAsk />
      <Composer />
      <NotebookDrawer />
    </>
  );
}
