// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// What a mission's MDX can use: predictions, designs, reveals gated on them, the machine,
// invariants, code anchors, and the two interactive steps (the safety-catch lab, the transfer).

import { createContext, type ReactNode, useContext, useEffect, useMemo, useRef, useState } from "react";
import { evidence, loadTrace } from "../data/load.ts";
import { here } from "../data/place.ts";
import { record, useProgress } from "../data/progress.ts";
import type { Trace } from "../data/types.ts";
import { SymbolSearch } from "../index/Search.tsx";
import { SceneStepper } from "../missions/pagePilot/Machine.tsx";
import { predictions, type PredictionId } from "../missions/pagePilot/predictions.ts";
import { ruleMutants } from "../missions/pagePilot/rules.ts";
import type { Scene } from "../missions/pagePilot/scene.ts";
import { transferTasks } from "../missions/pagePilot/transfer.ts";
import { TraceLegend, TracePlayer } from "../trace/TracePlayer.tsx";
import { AskButton } from "../notebook/Notebook.tsx";
import { CodeRef } from "./code.tsx";
import { EvidenceItem } from "./evidence.tsx";

export { Ref } from "./code.tsx";
export { DeadEndRef, Inv } from "./evidence.tsx";

/** The mission a step belongs to, which keys what the reader answered. */
export const MissionContext = createContext("page-pilot");

function useKey(id: string) {
  return `${useContext(MissionContext)}/${id}`;
}

export function Predict({ id }: { id: PredictionId }) {
  const key = useKey(id);
  const [progress, update] = useProgress();
  const prediction = predictions[id];
  const chosen = progress.answers[key];
  const answered = chosen !== undefined;
  return (
    <div className="predict">
      <div className="row" style={{ justifyContent: "space-between" }}>
        <span className="eyebrow">Predict before you look</span>
        <AskButton quote={prediction.question} />
      </div>
      <div className="predict-q">{prediction.question}</div>
      <div className="options">
        {prediction.options.map((option, i) => {
          const state = !answered ? "" : option.right ? "right" : i === chosen ? "wrong" : "";
          return (
            <button
              key={i}
              type="button"
              className={`option ${state}`}
              disabled={answered}
              onClick={() => {
                update((p) => ({ ...p, answers: { ...p.answers, [key]: i } }));
                record({
                  kind: "prediction",
                  question: prediction.question,
                  chose: option.text,
                  right: option.right,
                  answer: prediction.options.find((o) => o.right)?.text ?? "",
                  place: here(),
                });
              }}
            >
              <span>
                {answered && i === chosen && (
                  <span className={`verdict ${option.right ? "right" : "wrong"}`}>{option.right ? "Right · " : "Not quite · "}</span>
                )}
                {option.text}
              </span>
              {answered && (option.right || i === chosen) && <span className="why">{option.why}</span>}
            </button>
          );
        })}
      </div>
    </div>
  );
}

/** Its children once the reader has answered prediction or design `id`. */
export function After({ id, children }: { id: string; children: ReactNode }) {
  const key = useKey(id);
  const [progress] = useProgress();
  const done = progress.answers[key] !== undefined || progress.designs[key] !== undefined;
  return done ? <>{children}</> : null;
}

export function Design({ id, children }: { id: string; children: ReactNode }) {
  const key = useKey(id);
  const [progress, update] = useProgress();
  const [draft, setDraft] = useState("");
  const promptText = useRef<HTMLDivElement>(null);
  const kept = progress.designs[key];
  return (
    <div className="predict">
      <span className="eyebrow">Design it first</span>
      <div className="predict-q" ref={promptText}>
        {children}
      </div>
      {kept === undefined ? (
        <>
          <textarea
            id={`design-${id}`}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="Steps, in order, with the locks they hold…"
          />
          <div className="row">
            <button
              type="button"
              className="btn primary"
              disabled={draft.trim().length < 20}
              onClick={() => {
                update((p) => ({ ...p, designs: { ...p.designs, [key]: draft.trim() } }));
                record({ kind: "design", prompt: promptText.current?.innerText ?? id, text: draft.trim(), place: here() });
              }}
            >
              Commit my design, then show fxdax’s
            </button>
            <span className="muted" style={{ fontSize: "0.9rem" }}>
              A few lines are enough. Being wrong first is what makes the answer stick.
            </span>
          </div>
        </>
      ) : (
        <blockquote className="panel" style={{ margin: 0, whiteSpace: "pre-wrap" }}>
          <span className="eyebrow">Yours</span>
          <br />
          {kept}
        </blockquote>
      )}
    </div>
  );
}

export function Scenes({ scenes }: { scenes: Scene[] }) {
  return <SceneStepper scenes={scenes} />;
}

/** DaxCow's rules dax.c keeps another way, so that guest/sim has no mutant of them (the spec's header). */
const KEPT_OTHERWISE = new Set(["keepUntilSealed", "rebindAfterSeal"]);

/** Remove one of a spec's rules; guess what breaks; replay TLC's counterexample. */
export function CatchLab({ spec: specName }: { spec: string }) {
  const keyRule = useKey(`catch-rule-${specName}`);
  const [progress, update] = useProgress();
  const spec = evidence.specs.find((s) => s.spec === specName);
  const mutations = useMemo(() => evidence.mutations.filter((m) => m.spec === specName && m.removed.length === 1), [specName]);
  const [rule, setRule] = useState<string | null>(progress.catches[keyRule] ?? null);
  const [trace, setTrace] = useState<Trace | null>(null);
  const mutation = mutations.find((m) => m.removed[0] === rule);
  const guessKey = `${keyRule}/${rule}`;
  const guess = rule ? progress.catches[guessKey] : undefined;
  useEffect(() => {
    setTrace(null);
    if (!mutation || guess === undefined) return;
    let live = true;
    void loadTrace(mutation.spec, mutation.name)?.then((t) => live && setTrace(t));
    return () => {
      live = false;
    };
  }, [mutation, guess]);
  if (!spec) return null;
  const candidates = [...new Set(mutations.map((m) => m.expect))];
  return (
    <div className="lab">
      <div className="rules" role="group" aria-label="Rules to remove">
        {mutations.map((m) => (
          <button
            key={m.name}
            type="button"
            className="rule"
            aria-pressed={rule === m.removed[0]}
            onClick={() => {
              setRule(m.removed[0]);
              update((p) => ({ ...p, catches: { ...p.catches, [keyRule]: m.removed[0] } }));
            }}
          >
            <b>{m.removed[0]}</b>
            <span>{spec.rules[m.removed[0]]}</span>
          </button>
        ))}
      </div>
      {mutation && (
        <div className="predict">
          <span className="eyebrow">Without {mutation.removed[0]}, which invariant breaks first?</span>
          <div className="options">
            {candidates.map((inv) => {
              const state = guess === undefined ? "" : inv === mutation.expect ? "right" : inv === guess ? "wrong" : "";
              return (
                <button
                  key={inv}
                  type="button"
                  className={`option ${state}`}
                  disabled={guess !== undefined}
                  onClick={() => {
                    update((p) => ({ ...p, catches: { ...p.catches, [guessKey]: inv } }));
                    record({ kind: "catch", rule: mutation.removed[0], guessed: inv, actual: mutation.expect, place: here() });
                  }}
                >
                  <b style={{ fontFamily: "var(--mono)" }}>{inv}</b>
                  <span className="why">{spec.definitions[inv] ?? ""}</span>
                </button>
              );
            })}
          </div>
        </div>
      )}
      {mutation && guess !== undefined && (
        <>
          <p>
            <b>What TLC found.</b> {mutation.story}
          </p>
          <TraceLegend spec={specName} />
          {trace ? <TracePlayer trace={trace} spec={spec} /> : <p className="muted">Loading the trace…</p>}
          <div className="evidence">
            <span className="eyebrow">The same mistake in dax.c</span>
            {(ruleMutants[mutation.removed[0]] ?? []).length === 0 ? (
              <p className="muted">
                {KEPT_OTHERWISE.has(mutation.removed[0])
                  ? "No mutant of dax.c stands for this rule. The spec says why: dax.c keeps it another way. A rebind maps only what Fxfs says is sealed, and leaves a page private on the host’s EPERM."
                  : "No mutant in guest/mutants stands for this rule yet."}
              </p>
            ) : (
              ruleMutants[mutation.removed[0]].map((file) => <EvidenceItem key={file} item={{ kind: "mutant", file }} />)
            )}
          </div>
        </>
      )}
    </div>
  );
}

function elapsed(ms: number): string {
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

export function Transfer() {
  const mission = useContext(MissionContext);
  const [progress, update] = useProgress();
  const [now, setNow] = useState(Date.now());
  const [miss, setMiss] = useState<string | null>(null);
  const active = transferTasks.find((t) => {
    const r = progress.transfer[`${mission}/${t.id}`];
    return r && !r.found;
  });
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(timer);
  }, [active]);
  return (
    <div className="lab">
      {transferTasks.map((task) => {
        const key = `${mission}/${task.id}`;
        const run = progress.transfer[key];
        return (
          <div key={task.id} className="task panel">
            <span className="eyebrow">Find it</span>
            <div className="predict-q">{task.question}</div>
            {!run && (
              <div className="row">
                <button
                  type="button"
                  className="btn primary"
                  disabled={active !== undefined}
                  onClick={() => update((p) => ({ ...p, transfer: { ...p.transfer, [key]: { started: Date.now() } } }))}
                >
                  Start the clock
                </button>
              </div>
            )}
            {run && !run.found && (
              <>
                <div className="row" style={{ justifyContent: "space-between" }}>
                  <span className="timer num">{elapsed(now - run.started)}</span>
                  <span className="muted" style={{ fontSize: "0.9rem" }}>
                    Search, then pick the symbol. Or find it with rg in your checkout and search its name here.
                  </span>
                </div>
                {miss && <p className="verdict wrong">{miss}</p>}
                <SymbolSearch
                  autoFocus
                  placeholder="Search for the code that does it"
                  onPick={(sym) => {
                    record({
                      kind: "find",
                      task: task.question,
                      picked: `${sym.path} ${sym.container ? `${sym.container}::` : ""}${sym.name}`,
                      right: task.accepts(sym),
                      ms: Date.now() - run.started,
                      place: here({ code: { path: sym.path, snippet: sym.signature } }),
                    });
                    if (task.accepts(sym)) {
                      setMiss(null);
                      update((p) => ({
                        ...p,
                        transfer: { ...p.transfer, [key]: { ...run, found: Date.now(), picked: `${sym.path} ${sym.name}` } },
                      }));
                    } else setMiss(`Not that one: ${sym.name} in ${sym.path}. ${task.hint}`);
                  }}
                />
              </>
            )}
            {run?.found && (
              <>
                <p className="done num">
                  Found in {elapsed(run.found - run.started)}: {run.picked}
                </p>
                <CodeRef path={task.answer.path} snippet={task.answer.snippet} note={task.answer.note} origin="fxdax" />
              </>
            )}
          </div>
        );
      })}
    </div>
  );
}
