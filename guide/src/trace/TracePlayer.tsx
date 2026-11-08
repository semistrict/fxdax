// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { Fragment, useEffect, useState } from "react";
import type { Spec, Trace } from "../data/types.ts";

// A TLC counterexample, step by step: each state's values, what changed, and what the action that
// led there is, in the spec's own words. Specs/dax-cow gets a column per page and per task;
// any other spec, a column per variable.

type Values = Record<string, unknown>;

const SPECIAL: Record<number, string> = { [-1]: "garbage", [-2]: "zeros", [-3]: "torn" };

function version(data: unknown): string {
  return typeof data === "number" ? (SPECIAL[data] ?? `v${data}`) : String(data);
}

function set(value: unknown): string {
  return Array.isArray(value) ? `{${value.join(", ")}}` : String(value);
}

interface Column {
  key: string;
  label: string;
  cell: (values: Values) => string;
}

function daxCowColumns(pages: number): Column[] {
  const page = (name: string, p: number) => (v: Values) => (v[name] as unknown[])[p];
  // Who is doing what first, then the guest's sets, then each page's state.
  const cols: Column[] = [
    { key: "storer", label: "storer", cell: (v) => (v.spc === "idle" ? "idle" : `${v.sWay} p${v.sPage}: ${v.spc}`) },
    { key: "other", label: "other task", cell: (v) => (v.wpc === "idle" ? "idle" : `${v.wOp}: ${v.wpc}`) },
    { key: "sem", label: "semaphore", cell: (v) => (v.semX !== "none" ? `excl ${v.semX}` : (v.semR as string[]).length ? `shared ${set(v.semR)}` : "free") },
    { key: "dirty", label: "dirty", cell: (v) => set(v.dirty) },
    { key: "wb", label: "writeback", cell: (v) => set(v.wb) },
    { key: "sealing", label: "until seal", cell: (v) => set(v.sealing) },
  ];
  for (let p = 0; p < pages; p++) {
    cols.push(
      {
        key: `win${p}`,
        label: `window p${p + 1}`,
        cell: (v) => {
          const w = page("win", p)(v) as { kind: string; data: number };
          return `${w.kind} ${version(w.data)}`;
        },
      },
      { key: `pte${p}`, label: `PTE p${p + 1}`, cell: (v) => String(page("pte", p)(v)) },
      {
        key: `file${p}`,
        label: `Fxfs p${p + 1}`,
        cell: (v) => {
          const f = page("file", p)(v) as { state: string; data: number };
          return `${f.state} ${version(f.data)}`;
        },
      },
      { key: `durable${p}`, label: `durable p${p + 1}`, cell: (v) => version(page("durable", p)(v)) },
      { key: `latest${p}`, label: `stored p${p + 1}`, cell: (v) => version(page("latest", p)(v)) },
    );
  }
  return cols;
}

function genericColumns(vars: string[]): Column[] {
  return vars.map((name) => ({ key: name, label: name, cell: (v) => JSON.stringify(v[name]) }));
}

/** What an action does, in the spec's own comment, or ours where the spec has none. */
function gloss(spec: Spec, action: string, after: Values): string {
  if (spec.spec === "dax-cow") {
    if (action === "SStart") return `The storer starts a ${after.sWay} to page ${after.sPage}.`;
    if (action === "WStart") return `The other task starts a ${after.wOp}.`;
  }
  const own = spec.definitions[action];
  if (own) return own;
  return FALLBACK[action] ?? "";
}

// DaxCow's actions without a comment of their own, from their bodies.
const FALLBACK: Record<string, string> = {
  Init: "The file as read in: each page a sealed extent's, mapped read-only.",
  SLock: "The storer takes the file's semaphore shared.",
  WSwapLock: "Writeback takes the file's semaphore: exclusively, under the rule swapExclusive; shared without it.",
  WWp: "Writeback write-protects the pages' PTEs, after its swap.",
  WUnlock: "The other task lets go of the semaphore; an fsync goes on to its seal.",
  WRbLock: "A rebind takes the file's semaphore: exclusively, under the rule rebindExclusive; shared without it.",
  WPtLock: "A punch takes the file's semaphore exclusively.",
  Seal: "A seal from elsewhere (the journal, another file's fsync): open extents sealed, commits made durable, replaced extents freed.",
};

export function TracePlayer({ trace, spec }: { trace: Trace; spec: Spec }) {
  const [at, setAt] = useState(0);
  useEffect(() => setAt(0), [trace]);
  const pages = Array.isArray(trace.states[0].values.win) ? (trace.states[0].values.win as unknown[]).length : 0;
  const columns = trace.spec === "dax-cow" ? daxCowColumns(pages) : genericColumns(trace.vars);
  const last = trace.states.length - 1;
  const now = trace.states[at];
  const cells = trace.states.map((s) => columns.map((c) => c.cell(s.values)));
  return (
    <div className="trace">
      <div className="row" style={{ justifyContent: "space-between" }}>
        <span className="eyebrow num">
          TLC {trace.tlc} counterexample, recorded at <code className="hash">{trace.commit}</code> · step {at} of {last}
        </span>
        <div className="row">
          <button type="button" className="btn" onClick={() => setAt(0)} disabled={at === 0}>
            Start
          </button>
          <button type="button" className="btn" onClick={() => setAt(at - 1)} disabled={at === 0}>
            Back
          </button>
          <button type="button" className="btn primary" onClick={() => setAt(at + 1)} disabled={at === last}>
            Next step
          </button>
          <button type="button" className="btn" onClick={() => setAt(last)} disabled={at === last}>
            End
          </button>
        </div>
      </div>
      <div className="gloss panel">
        <b>{now.action}</b>
        <span>{gloss(spec, now.action, now.values)}</span>
      </div>
      <div className="trace-table">
        <table>
          <thead>
            <tr>
              <th className="pin">#</th>
              <th className="pin pin-2">action</th>
              {columns.map((c) => (
                <th key={c.key}>{c.label}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {trace.states.map((s, i) => (
              <tr
                key={i}
                className={[i === at ? "now" : "", i > at ? "future" : "", i === last && at === last ? "violation" : ""].join(" ")}
                onClick={() => setAt(i)}
                style={{ cursor: "pointer" }}
              >
                <td className="num pin">{i}</td>
                <td className="pin pin-2">{s.action}</td>
                {columns.map((c, j) => (
                  <td key={c.key} className={i > 0 && cells[i][j] !== cells[i - 1][j] ? "changed" : ""}>
                    {cells[i][j]}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {at === last && (
        <div className="verdict-box">
          <b>
            <code>{trace.violated}</code> is violated.
          </b>
          {spec.definitions[trace.violated] && <span>{spec.definitions[trace.violated]}</span>}
          {spec.invariants[trace.violated] && <pre>{spec.invariants[trace.violated]}</pre>}
        </div>
      )}
    </div>
  );
}

export function TraceLegend({ spec }: { spec: string }) {
  if (spec !== "dax-cow") return null;
  return (
    <p className="muted" style={{ fontSize: "0.92rem" }}>
      {[
        ["window", "what the host maps there, and its version"],
        ["Fxfs", "the file's committed block"],
        ["durable", "what a crash would show"],
        ["stored", "the last version a store or punch completed"],
      ].map(([k, v], i) => (
        <Fragment key={k}>
          {i > 0 && "; "}
          <b>{k}</b>: {v}
        </Fragment>
      ))}
      . Changed cells are highlighted. Click a row to jump to it.
    </p>
  );
}
