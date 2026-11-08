// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { invariants } from "../content/invariants.ts";
import { layers } from "../content/layers.ts";
import type { EvidenceRef, Invariant } from "../content/types.ts";
import { evidence, hasTrace } from "../data/load.ts";
import { AskButton } from "../notebook/Notebook.tsx";
import { Ref, WholeFile } from "./code.tsx";

export const STANDING_LABEL = { checked: "checked", stated: "stated" } as const;

export function layerName(id: string): string {
  return layers.find((l) => l.id === id)?.name ?? id;
}

export function docOf(id: string) {
  return evidence.docs.find((d) => d.id === id);
}

export function DocLink({ id, heading }: { id: string; heading?: string }) {
  const doc = docOf(id);
  if (!doc) return <span>{id}</span>;
  return (
    <a href={`#${doc.id}`}>
      {doc.id}: {doc.title}
      {heading ? `, “${heading}”` : ""}
    </a>
  );
}

export function DeadEndRef({ file }: { file: string }) {
  const dead = evidence.deadEnds.find((d) => d.file === file);
  if (!dead) return null;
  return (
    <div className="evidence">
      <details open>
        <summary>
          Dead end: {dead.title}
        </summary>
        <div className="prose" style={{ gap: 8, paddingBlock: 8 }}>
          {dead.idea && (
            <p>
              <b>Idea.</b> {dead.idea}
            </p>
          )}
          {dead.result && (
            <p>
              <b>Result.</b> {dead.result}
            </p>
          )}
          {dead.why && (
            <p>
              <b>Why it’s dead.</b> {dead.why}
            </p>
          )}
          <a href={`#dead-${file.replace(/\.md$/, "")}`}>Read dead-ends/{file}</a>
        </div>
      </details>
    </div>
  );
}

export function EvidenceItem({ item }: { item: EvidenceRef }) {
  switch (item.kind) {
    case "doc":
      return (
        <div className="muted" style={{ fontSize: "0.92rem" }}>
          Stated in <DocLink id={item.id} heading={item.heading} />
        </div>
      );
    case "deadEnd":
      return <DeadEndRef file={item.file} />;
    case "mutation": {
      const m = evidence.mutations.find((x) => x.spec === item.spec && x.name === item.name);
      if (!m) return null;
      return (
        <details>
          <summary>
            TLA+: without <code>{m.removed.join(", ") || "?"}</code>, TLC finds <code>{m.expect}</code> violated
            {hasTrace(m.spec, m.name) ? " (trace in Page Pilot)" : ""}
          </summary>
          <p style={{ paddingBlock: 6 }}>{m.story}</p>
          <WholeFile path={`specs/${m.spec}/mutations/${m.name}.cfg`} />
        </details>
      );
    }
    case "mutant": {
      const m = evidence.mutants.find((x) => x.file === item.file);
      if (!m) return null;
      return (
        <details>
          <summary>
            Past bug, put back: <code>{m.file}</code>, killed by <code>{m.killer}</code> ({m.kernel})
          </summary>
          <p style={{ paddingBlock: 6 }}>{m.story}</p>
          <pre>{m.patch}</pre>
          <code>guest/mutants/{m.file}</code>
        </details>
      );
    }
  }
}

export function InvariantCard({ inv, compact = false }: { inv: Invariant; compact?: boolean }) {
  return (
    <article className={`inv${compact ? " compact" : ""}`} style={{ "--layer": `var(--l-${inv.layer})` } as React.CSSProperties}>
      <div className="row" style={{ justifyContent: "space-between" }}>
        <span className="eyebrow">{layerName(inv.layer)}</span>
        <span className="row" style={{ gap: 8 }}>
          <AskButton quote={inv.claim} />
          <span className={`chip standing-${inv.standing}`}>{STANDING_LABEL[inv.standing]}</span>
        </span>
      </div>
      <div className="inv-claim">{inv.claim}</div>
      {!compact && <p className="muted">{inv.why}</p>}
      {!compact && (
        <>
          <div className="refs">
            {inv.enforcedBy.map((id) => (
              <Ref key={id} id={id} />
            ))}
          </div>
          <div className="evidence">
            {inv.evidence.map((item, i) => (
              <EvidenceItem key={i} item={item} />
            ))}
          </div>
        </>
      )}
      {compact && (
        <a href={`#invariants-${inv.id}`} style={{ fontSize: "0.9rem" }}>
          Code and evidence
        </a>
      )}
    </article>
  );
}

export function Inv({ id }: { id: string }) {
  const inv = invariants.find((i) => i.id === id);
  return inv ? <InvariantCard inv={inv} compact /> : null;
}
