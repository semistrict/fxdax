// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { meta } from "../data/load.ts";
import { codeAt, here } from "../data/place.ts";
import { type Entry, forget, type Place, record, update, useProgress } from "../data/progress.ts";
import { notebookText } from "./export.ts";

// Comment on anything, ask about anything: select text anywhere, or press a card's "Ask", and the
// composer opens on what was selected, where. Everything goes into the notebook, which copies out
// as one blob for Claude.

let composing: Place | null = null;
let drawerOpen = false;
const listeners = new Set<() => void>();
const notify = () => listeners.forEach((l) => l());
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

export function openComposer(place: Place) {
  composing = place;
  notify();
}

function closeComposer() {
  composing = null;
  notify();
}

export function toggleNotebook(open = !drawerOpen) {
  drawerOpen = open;
  notify();
}

/** "Ask" on a card: a question or a note about it, with its quote and code. */
export function AskButton({ quote, code, label = "Ask" }: { quote?: string; code?: Place["code"]; label?: string }) {
  return (
    <button type="button" className="ask" onClick={(e) => openComposer(here({ quote, code: code ?? codeAt(e.currentTarget) }))}>
      {label}
    </button>
  );
}

/** A floating "Ask about this" by any selection of the page's text. */
export function SelectionAsk() {
  const [at, setAt] = useState<{ x: number; y: number; text: string; code: Place["code"] } | null>(null);
  useEffect(() => {
    const onUp = (event: MouseEvent | KeyboardEvent) => {
      if ((event.target as Element | null)?.closest?.(".composer, .notebook, .sel-ask")) return;
      window.setTimeout(() => {
        const sel = getSelection();
        const text = sel?.toString().trim() ?? "";
        if (!sel || sel.isCollapsed || text.length < 2 || !sel.rangeCount) return setAt(null);
        const node = sel.anchorNode instanceof Element ? sel.anchorNode : sel.anchorNode?.parentElement ?? null;
        if (!node?.closest("main")) return setAt(null);
        const rect = sel.getRangeAt(0).getBoundingClientRect();
        setAt({ x: Math.min(rect.right, innerWidth - 150), y: rect.bottom + 6, text: text.slice(0, 1200), code: codeAt(node) });
      }, 0);
    };
    addEventListener("mouseup", onUp);
    addEventListener("keyup", onUp);
    return () => {
      removeEventListener("mouseup", onUp);
      removeEventListener("keyup", onUp);
    };
  }, []);
  if (!at) return null;
  return (
    <button
      type="button"
      className="sel-ask"
      style={{ left: at.x, top: at.y }}
      onMouseDown={(e) => e.preventDefault()}
      onClick={() => {
        openComposer(here({ quote: at.text, code: at.code }));
        setAt(null);
      }}
    >
      Ask about this
    </button>
  );
}

export function Composer() {
  const place = useSyncExternalStore(subscribe, () => composing);
  const [kind, setKind] = useState<"question" | "note">("question");
  const [text, setText] = useState("");
  const box = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    if (place) {
      setText("");
      box.current?.focus();
    }
  }, [place]);
  if (!place) return null;
  const save = () => {
    if (!text.trim()) return;
    record({ kind, text: text.trim(), place });
    closeComposer();
  };
  return (
    <div className="composer-scrim" onClick={closeComposer}>
      <div className="composer panel" role="dialog" aria-label="Ask or note" onClick={(e) => e.stopPropagation()}>
        <span className="eyebrow">{place.view}</span>
        {place.code && <code className="ref-path">{place.code.path}</code>}
        {place.quote && <blockquote className="composer-quote">{place.quote}</blockquote>}
        <div className="marks" role="group" aria-label="Kind">
          <button type="button" aria-pressed={kind === "question"} onClick={() => setKind("question")}>
            Question
          </button>
          <button type="button" aria-pressed={kind === "note"} onClick={() => setKind("note")}>
            Note
          </button>
        </div>
        <textarea
          id="composer-text"
          ref={box}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) save();
            if (e.key === "Escape") closeComposer();
          }}
          placeholder={kind === "question" ? "What do you want to know about this?" : "What do you think, or want to remember?"}
        />
        <div className="row" style={{ justifyContent: "space-between" }}>
          <span className="muted" style={{ fontSize: "0.85rem" }}>
            Goes in your notebook, with where you were. ⌘↵ saves.
          </span>
          <div className="row">
            <button type="button" className="btn" onClick={closeComposer}>
              Cancel
            </button>
            <button type="button" className="btn primary" disabled={!text.trim()} onClick={save}>
              Add to notebook
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

export function NotebookButton() {
  const [progress] = useProgress();
  const fresh = progress.notebook.length - progress.copied;
  return (
    <button type="button" className="btn notebook-btn" onClick={() => toggleNotebook()}>
      Notebook{fresh > 0 ? ` · ${fresh} new` : ""}
    </button>
  );
}

function summary(entry: Entry): { tag: string; tone: "good" | "bad" | "plain"; text: string } {
  switch (entry.kind) {
    case "prediction":
      return { tag: entry.right ? "Right" : "Wrong", tone: entry.right ? "good" : "bad", text: `${entry.question} You: “${entry.chose}”` };
    case "design":
      return { tag: "Design", tone: "plain", text: entry.text };
    case "catch":
      return {
        tag: entry.guessed === entry.actual ? "Right" : "Wrong",
        tone: entry.guessed === entry.actual ? "good" : "bad",
        text: `Without ${entry.rule}: you said ${entry.guessed}; TLC: ${entry.actual}`,
      };
    case "find":
      return { tag: entry.right ? "Found" : "Missed", tone: entry.right ? "good" : "bad", text: `${entry.task} → ${entry.picked}` };
    case "mark":
      return { tag: "Mark", tone: "plain", text: `${entry.item}: ${entry.mark}` };
    case "question":
      return { tag: "Question", tone: "plain", text: entry.text };
    case "note":
      return { tag: "Note", tone: "plain", text: entry.text };
  }
}

export function NotebookDrawer() {
  const open = useSyncExternalStore(subscribe, () => drawerOpen);
  const [progress] = useProgress();
  const [copied, setCopied] = useState<string | null>(null);
  if (!open) return null;
  const fresh = progress.notebook.length - progress.copied;
  const copy = (from: number) => {
    const text = notebookText(progress, from, meta.commit);
    const done = () => {
      update((p) => ({ ...p, copied: p.notebook.length }));
      setCopied("Copied. Paste it into Claude Code in your fxdax checkout.");
    };
    navigator.clipboard.writeText(text).then(done, () => {
      const area = document.getElementById("notebook-blob") as HTMLTextAreaElement | null;
      if (area) {
        area.value = text;
        area.hidden = false;
        area.select();
      }
      setCopied("Your browser refused the clipboard: the text is selected below, copy it.");
    });
  };
  return (
    <aside className="notebook" aria-label="Notebook">
      <div className="row" style={{ justifyContent: "space-between" }}>
        <h2>Notebook</h2>
        <button type="button" className="btn" onClick={() => toggleNotebook(false)}>
          Close
        </button>
      </div>
      <p className="muted" style={{ fontSize: "0.92rem" }}>
        Everything you answer, design, find, mark and ask, in order. Copy it out and paste it into Claude Code, so it
        knows where you are.
      </p>
      <div className="row">
        <button type="button" className="btn primary" disabled={fresh === 0} onClick={() => copy(progress.copied)}>
          Copy {fresh} new for Claude
        </button>
        <button type="button" className="btn" disabled={progress.notebook.length === 0} onClick={() => copy(0)}>
          Copy all {progress.notebook.length}
        </button>
      </div>
      {copied && <p className="done" style={{ fontSize: "0.9rem" }}>{copied}</p>}
      <textarea id="notebook-blob" hidden readOnly />
      <ol className="entries" reversed>
        {[...progress.notebook].reverse().map((entry, i) => {
          const s = summary(entry);
          const isNew = progress.notebook.length - 1 - i >= progress.copied;
          return (
            <li key={entry.at} className={`entry ${s.tone}${isNew ? " new" : ""}`}>
              <div className="row" style={{ justifyContent: "space-between", gap: 6 }}>
                <span className="entry-tag">{s.tag}</span>
                <a href={`#${entry.place.hash}`} className="entry-where" onClick={() => toggleNotebook(false)}>
                  {entry.place.view}
                </a>
              </div>
              {entry.place.quote && <blockquote className="composer-quote">{entry.place.quote}</blockquote>}
              <div className="entry-text">{s.text}</div>
              <button type="button" className="entry-forget" onClick={() => forget(entry.at)} aria-label="Remove this entry">
                Remove
              </button>
            </li>
          );
        })}
      </ol>
      {progress.notebook.length === 0 && (
        <p className="muted">
          Nothing yet. Answer a prediction in Page Pilot, mark a layer in the Atlas, or select any text and press “Ask
          about this”.
        </p>
      )}
    </aside>
  );
}
