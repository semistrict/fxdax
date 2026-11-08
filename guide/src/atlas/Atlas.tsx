// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { Fragment, useMemo, useState } from "react";
import { invariants } from "../content/invariants.ts";
import { layers } from "../content/layers.ts";
import type { LayerId } from "../content/types.ts";
import { useIndex } from "../data/load.ts";
import { here, setHere } from "../data/place.ts";
import { type Mark, record, useProgress } from "../data/progress.ts";
import { AskButton } from "../notebook/Notebook.tsx";
import type { Origin } from "../data/types.ts";
import { CopyButton, ORIGIN_LABEL, Ref } from "../ui/code.tsx";
import { DocLink, InvariantCard } from "../ui/evidence.tsx";
import { learningPrompt } from "./prompt.ts";

// What crosses between each layer and the next.
const HOPS: Record<string, string> = {
  guest: "Fxfs's device calls: read, write, free, commit, map",
  driver: "virtqueues: READ WRITE FREE COMMIT MAP UNMAP PRIVATE",
  vmm: "Unix socket; the daemon's memfds, mapped",
  daemon: "pages: cache, windows, private copies",
  memory: "misses read from the store; writes go to it",
  disk: "framed segments uploaded; extents fetched",
};

const MARKS: [Mark, string][] = [
  ["know", "Know it"],
  ["fuzzy", "Fuzzy"],
  ["unknown", "No idea"],
];

function MarkButtons({ id, name }: { id: string; name: string }) {
  const [progress, update] = useProgress();
  const mark = progress.marks[id];
  return (
    <div className="marks" role="group" aria-label="How well you know it">
      {MARKS.map(([value, label]) => (
        <button
          key={value}
          type="button"
          aria-pressed={mark === value}
          onClick={() => {
            const cleared = mark === value;
            update((p) => {
              const marks = { ...p.marks };
              if (cleared) delete marks[id];
              else marks[id] = value;
              return { ...p, marks };
            });
            record({ kind: "mark", item: name, mark: cleared ? "cleared" : value, place: here({ quote: name }) });
          }}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

const MARK_GLYPH: Record<Mark, string> = { know: "known", fuzzy: "fuzzy", unknown: "unknown" };

function Provenance() {
  const index = useIndex();
  const counts = useMemo(() => {
    const files = new Map<Origin, number>();
    const symbols = new Map<Origin, number>();
    for (const f of index?.files ?? []) files.set(f.origin, (files.get(f.origin) ?? 0) + 1);
    for (const s of index?.symbols ?? []) symbols.set(s.origin, (symbols.get(s.origin) ?? 0) + 1);
    return { files, symbols };
  }, [index]);
  if (!index) return <p className="muted">Loading the index…</p>;
  return (
    <div className="stats">
      {(["fxdax", "ported", "fuchsia-changed", "fuchsia", "stand-in"] as Origin[]).map((o) => (
        <div key={o} className="stat">
          <b className={`o-${o} num`}>{(counts.symbols.get(o) ?? 0).toLocaleString()}</b>
          <span>
            {ORIGIN_LABEL[o]} symbols, {counts.files.get(o) ?? 0} files
          </span>
        </div>
      ))}
    </div>
  );
}

export function Atlas() {
  const [selected, setSelected] = useState<LayerId>("guest");
  const [progress] = useProgress();
  const layer = layers.find((l) => l.id === selected) ?? layers[0];
  setHere(`Atlas, the ${layer.name} layer`, "atlas");
  const layerInvariants = invariants.filter((i) => i.layer === layer.id);
  const prompt = learningPrompt(progress.marks);
  return (
    <>
      <div className="lede">
        <span className="eyebrow">Atlas</span>
        <h1>The stack, top to bottom</h1>
        <p>
          A guest’s Fxfs keeps its files as extents, immutable once sealed. The host keeps them for it, maps sealed ones
          straight into the guest’s memory, and backs everything up to a bucket. Pick a layer. Mark what you know; the
          page writes the prompt that asks Claude Code to teach you the rest, from this repo’s code.
        </p>
      </div>
      <div className="atlas">
        <div className="stack" aria-label="The stack">
          {layers.map((l, i) => (
            <Fragment key={l.id}>
              <button
                type="button"
                className="band"
                style={{ "--layer": `var(--l-${l.id})`, gridRow: i * 2 + 1 } as React.CSSProperties}
                aria-pressed={l.id === selected}
                onClick={() => setSelected(l.id)}
              >
                <span className="band-name">
                  {l.name}
                  {progress.marks[`layer:${l.id}`] && (
                    <span className="band-mark muted">{MARK_GLYPH[progress.marks[`layer:${l.id}`]]}</span>
                  )}
                </span>
                <span className="band-what">{l.what}</span>
              </button>
              {HOPS[l.id] && (
                <div className="hop" style={{ gridRow: i * 2 + 2 }}>
                  {HOPS[l.id]}
                </div>
              )}
            </Fragment>
          ))}
          <div className="bypass" aria-hidden="true">
            <span>DAX: loads hit these pages</span>
          </div>
        </div>
        <section className="layer-detail panel" aria-live="polite" style={{ borderTop: `4px solid var(--l-${layer.id})` }}>
          <div className="row" style={{ justifyContent: "space-between" }}>
            <h2>{layer.name}</h2>
            <AskButton quote={`The ${layer.name} layer: ${layer.what}`} />
            <MarkButtons id={`layer:${layer.id}`} name={`the ${layer.name} layer`} />
          </div>
          <p>{layer.role}</p>
          <p className="muted">{layer.provenance}</p>
          <div className="row" style={{ gap: "6px 14px", fontSize: "0.92rem" }}>
            <span className="eyebrow">Docs</span>
            {layer.docs.map((d) => (
              <DocLink key={d} id={d} />
            ))}
          </div>
          <div className="row" style={{ gap: "6px 14px" }}>
            <span className="eyebrow">Where it lives</span>
            {layer.dirs.map((d) => (
              <code key={d}>{d}</code>
            ))}
          </div>
          <div className="refs">
            {layer.anchors.map((id) => (
              <Ref key={id} id={id} />
            ))}
          </div>
          {layerInvariants.length > 0 && (
            <>
              <h3>What holds here</h3>
              {layerInvariants.map((inv) => (
                <div key={inv.id} className="row" style={{ alignItems: "flex-start" }}>
                  <div style={{ flex: "1 1 320px", minWidth: 0 }}>
                    <InvariantCard inv={inv} compact />
                  </div>
                  <MarkButtons id={`inv:${inv.id}`} name={inv.claim} />
                </div>
              ))}
            </>
          )}
        </section>
      </div>
      <section className="panel" style={{ display: "grid", gap: 12 }} data-copy-target>
        <div className="row" style={{ justifyContent: "space-between" }}>
          <h2>Ask Claude Code to teach you the gaps</h2>
          {prompt && (
            <span className="ref-actions">
              <CopyButton text={prompt} label="Copy prompt" />
            </span>
          )}
        </div>
        {prompt ? (
          <pre className="prompt-out">{prompt}</pre>
        ) : (
          <p className="muted">
            Mark a layer or a claim “Fuzzy” or “No idea”, and the prompt appears here, naming those gaps with the files
            and snippets to start from. Paste it into Claude Code in your fxdax checkout.
          </p>
        )}
      </section>
      <section style={{ display: "grid", gap: 12 }}>
        <h2>Whose code it is</h2>
        <p className="muted">
          Nearly half the symbols are Fuchsia’s: Fxfs, its libraries, the runtime crates and the SDK’s zx types, copied
          from the revision the root Cargo.toml records. A file fxdax changed says how in its leading comment; the index
          labels each file so.
        </p>
        <Provenance />
      </section>
    </>
  );
}
