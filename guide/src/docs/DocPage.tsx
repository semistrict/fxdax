// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { marked } from "marked";
import { useEffect, useRef, useState } from "react";
import { fetchSource } from "../data/links.ts";
import { evidence } from "../data/load.ts";

// A design doc or a dead end, rendered from the guide's local copy of the repo's markdown. Mentions
// of other docs (doc-N) and dead ends' files link within the guide.

function withoutFrontMatter(text: string): string {
  return text.startsWith("---\n") ? text.slice(text.indexOf("\n---\n", 4) + 5) : text;
}

/** Links each `doc-N` the rendered text mentions to that doc's page. */
function linkDocs(root: HTMLElement) {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const found: Text[] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (/\bdoc-\d+\b/.test(node.nodeValue ?? "") && !node.parentElement?.closest("a, pre")) found.push(node as Text);
  }
  for (const node of found) {
    const parts = (node.nodeValue ?? "").split(/\b(doc-\d+)\b/);
    const fragment = document.createDocumentFragment();
    for (const part of parts) {
      if (/^doc-\d+$/.test(part) && evidence.docs.some((d) => d.id === part)) {
        const a = document.createElement("a");
        a.href = `#${part}`;
        a.textContent = part;
        fragment.append(a);
      } else fragment.append(part);
    }
    node.replaceWith(fragment);
  }
}

export function MarkdownPage({ path, eyebrow }: { path: string; eyebrow: string }) {
  const [html, setHtml] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const body = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let live = true;
    setHtml(null);
    fetchSource(path).then(
      // The repo's own markdown, rendered as its authors wrote it.
      (text) => live && setHtml(marked.parse(withoutFrontMatter(text), { async: false })),
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [path]);
  useEffect(() => {
    if (html !== null && body.current) linkDocs(body.current);
  }, [html]);
  if (error) return <p className="muted">The guide has no copy of {path}.</p>;
  return (
    <>
      <div className="lede">
        <span className="eyebrow">
          {eyebrow} · <code className="hash">{path}</code>
        </span>
      </div>
      {html === null ? <p className="muted">Loading…</p> : <div ref={body} className="doc" dangerouslySetInnerHTML={{ __html: html }} />}
    </>
  );
}

export function DocPage({ id }: { id: string }) {
  const doc = evidence.docs.find((d) => d.id === id);
  if (!doc) return <p className="muted">There is no {id}.</p>;
  return <MarkdownPage path={`backlog/docs/${doc.file}`} eyebrow={`Design doc ${doc.id}`} />;
}

export function DeadEndPage({ slug }: { slug: string }) {
  const dead = evidence.deadEnds.find((d) => d.file === `${slug}.md`);
  if (!dead) return <p className="muted">There is no dead end {slug}.</p>;
  return <MarkdownPage path={`dead-ends/${dead.file}`} eyebrow={dead.theory ? "Wrong theory" : "Dead end"} />;
}
