// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import hljs from "highlight.js/lib/core";
import c from "highlight.js/lib/languages/c";
import markdown from "highlight.js/lib/languages/markdown";
import rust from "highlight.js/lib/languages/rust";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { anchors, type AnchorId } from "../content/anchors.ts";
import { excerpt, fetchSource, rgCommand } from "../data/links.ts";
import { useIndex } from "../data/load.ts";
import { AskButton } from "../notebook/Notebook.tsx";
import type { Origin } from "../data/types.ts";

hljs.registerLanguage("rust", rust);
hljs.registerLanguage("c", c);
hljs.registerLanguage("markdown", markdown);

export const ORIGIN_LABEL: Record<Origin, string> = {
  fxdax: "fxdax",
  ported: "ported from Fuchsia",
  "fuchsia-changed": "Fuchsia, changed",
  fuchsia: "Fuchsia",
  "stand-in": "stand-in",
};

export function OriginChip({ origin }: { origin: Origin }) {
  return <span className={`chip o-${origin}`}>{ORIGIN_LABEL[origin]}</span>;
}

function language(path: string): string | null {
  if (path.endsWith(".rs")) return "rust";
  if (/\.[ch]$/.test(path)) return "c";
  if (path.endsWith(".md")) return "markdown";
  return null;
}

/** `text` highlighted as `path`'s language, one HTML string per line. */
function highlightLines(text: string, path: string): string[] {
  const lang = language(path);
  const escape = (s: string) => s.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
  if (!lang) return text.split("\n").map(escape);
  // highlight.js escapes the text it marks up. Its spans can cross lines: close and reopen them
  // at each line's end, so that each line stands alone.
  const html = hljs.highlight(text, { language: lang }).value;
  const out: string[] = [];
  const open: string[] = [];
  for (const line of html.split("\n")) {
    const prefix = open.join("");
    for (const tag of line.matchAll(/<span[^>]*>|<\/span>/g)) {
      if (tag[0] === "</span>") open.pop();
      else open.push(tag[0]);
    }
    out.push(prefix + line + "</span>".repeat(open.length));
  }
  return out;
}

export function Code({ text, path }: { text: string; path: string }) {
  const html = useMemo(() => highlightLines(text, path).join("\n"), [text, path]);
  return (
    <pre>
      <code dangerouslySetInnerHTML={{ __html: html }} />
    </pre>
  );
}

/** Copies `text`, falling back to selecting it where the clipboard refuses. */
export function CopyButton({ text, label }: { text: string; label: string }) {
  const [done, setDone] = useState<string | null>(null);
  return (
    <button
      type="button"
      onClick={(event) => {
        const button = event.currentTarget;
        navigator.clipboard.writeText(text).then(
          () => setDone("Copied"),
          () => {
            const range = document.createRange();
            const target = button.closest("[data-copy-target]")?.querySelector("pre") ?? button;
            range.selectNodeContents(target);
            getSelection()?.removeAllRanges();
            getSelection()?.addRange(range);
            setDone("Selected: copy it");
          },
        );
        window.setTimeout(() => setDone(null), 1600);
      }}
    >
      {done ?? label}
    </button>
  );
}

/**
 * The local source around `snippet` in `path`: what it is in, to the end of the block it opens,
 * with the snippet's lines marked; or the whole file, scrolled to it.
 */
export function SourceView({ path, snippet }: { path: string; snippet: string }) {
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [whole, setWhole] = useState(false);
  const marked = useRef<HTMLDivElement>(null);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let live = true;
    fetchSource(path).then(
      (t) => live && setText(t),
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [path]);
  const lines = useMemo(() => (text === null ? [] : highlightLines(text, path)), [text, path]);
  const range = useMemo(() => (text === null ? null : excerpt(text, snippet)), [text, snippet]);
  useLayoutEffect(() => {
    if (whole && marked.current && box.current) {
      box.current.scrollTop = marked.current.offsetTop - box.current.clientHeight / 3;
    }
  }, [whole]);
  if (error) return <p className="muted">The guide has no copy of {path}: rebuild it.</p>;
  if (text === null) return <p className="muted">Loading {path}…</p>;
  if (!range) return <p className="verdict wrong">The snippet is no longer in {path}: the guide needs rebuilding.</p>;
  const snippetLines = snippet.split("\n").length;
  const first = whole ? 0 : range.first;
  const last = whole ? lines.length - 1 : range.last;
  return (
    <div className="source">
      <div ref={box} className={`source-lines${whole ? " whole" : ""}`}>
        {lines.slice(first, last + 1).map((html, i) => {
          const n = first + i;
          const hit = n >= range.at && n < range.at + snippetLines;
          return (
            <div key={n} ref={n === range.at ? marked : undefined} className={`source-line${hit ? " hit" : ""}`}>
              <span className="source-n" aria-hidden="true">
                {n + 1}
              </span>
              <code dangerouslySetInnerHTML={{ __html: html || " " }} />
            </div>
          );
        })}
      </div>
      <div className="ref-actions">
        <button type="button" onClick={() => setWhole(!whole)}>
          {whole ? "Show just this part" : `Show all of ${path.split("/").pop()} (${lines.length} lines)`}
        </button>
      </div>
    </div>
  );
}

/** A whole file of the guide's local copy, with its path. */
export function WholeFile({ path }: { path: string }) {
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    void fetchSource(path).then((t) => live && setText(t));
    return () => {
      live = false;
    };
  }, [path]);
  return (
    <div style={{ display: "grid", gap: 4 }}>
      <code className="ref-path">{path}</code>
      {text === null ? <span className="muted">Loading…</span> : <Code text={text} path={path} />}
    </div>
  );
}

/** A place in the code, by path and snippet: what it is, its source, and the rg that finds it. */
export function CodeRef({
  path,
  snippet,
  note,
  origin,
  open = false,
}: {
  path: string;
  snippet: string;
  note?: string;
  origin: Origin;
  open?: boolean;
}) {
  const index = useIndex();
  const [shown, setShown] = useState(open);
  const sym = index?.bySignature.get(`${path}\n${snippet}`);
  return (
    <div
      className="ref"
      style={{ "--origin": `var(--o-${origin})` } as React.CSSProperties}
      data-copy-target
      data-code-path={path}
      data-code-snippet={snippet}
    >
      <div className="ref-head">
        <OriginChip origin={origin} />
        <span className="ref-path">{path}</span>
      </div>
      {shown ? <SourceView path={path} snippet={snippet} /> : <Code text={snippet} path={path} />}
      {note && <div className="ref-note">{note}</div>}
      {sym?.doc && note !== sym.doc && <div className="ref-doc">{sym.doc}</div>}
      <div className="ref-actions">
        <button type="button" onClick={() => setShown(!shown)} aria-expanded={shown}>
          {shown ? "Hide the code" : "Show the code"}
        </button>
        <CopyButton text={rgCommand(path, snippet)} label="Copy rg" />
        <CopyButton text={path} label="Copy path" />
        <AskButton code={{ path, snippet }} />
      </div>
    </div>
  );
}

/** An anchor of content/anchors.ts. */
export function Ref({ id, open = false }: { id: AnchorId; open?: boolean }) {
  const anchor = anchors[id];
  const index = useIndex();
  const origin = index?.files.find((f) => f.path === anchor.path)?.origin ?? originGuess(anchor.path);
  return <CodeRef path={anchor.path} snippet={anchor.snippet} note={anchor.note} origin={origin} open={open} />;
}

/** An anchor's origin before the index has loaded, from where its file is. */
function originGuess(path: string): Origin {
  return /^crates\/(fxfs|storage_device|fuchsia|zx|fidl)/.test(path) ? "fuchsia" : "fxdax";
}
