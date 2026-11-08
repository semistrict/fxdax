// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { invariants } from "../content/invariants.ts";
import { layers } from "../content/layers.ts";
import type { Entry, Mark, Progress } from "../data/progress.ts";

// The notebook as one markdown blob for Claude Code in a checkout of the repo: who the reader is
// and what to do with it, then what they got right and wrong, what they designed, what they mark
// known, and their questions and notes with where they were and what they were looking at.

function clock(ms: number): string {
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

function quote(text: string): string {
  return text
    .trim()
    .split("\n")
    .map((line) => `> ${line}`)
    .join("\n");
}

function where(entry: Entry): string {
  const parts = [`on ${entry.place.view}`];
  if (entry.place.code) parts.push(`at \`${entry.place.code.path}\`: \`${entry.place.code.snippet}\``);
  return parts.join(", ");
}

function itemName(id: string): string {
  const [kind, key] = id.split(":");
  if (kind === "layer") return `the ${layers.find((l) => l.id === key)?.name ?? key} layer`;
  const inv = invariants.find((i) => i.id === key);
  return inv ? `"${inv.claim}"` : id;
}

/**
 * The notebook's entries from `from` on, as markdown, with what the reader marks known as of now.
 * `commit` is the repo commit the guide was built from.
 */
export function notebookText(progress: Progress, from: number, commit: string): string {
  const entries = progress.notebook.slice(from);
  const of = <K extends Entry["kind"]>(...kinds: K[]) => entries.filter((e): e is Extract<Entry, { kind: K }> => kinds.includes(e.kind as K));
  const out: string[] = [
    `# My fxdax Flight School notebook`,
    "",
    `Exported ${new Date().toISOString().slice(0, 16).replace("T", " ")} UTC, from the guide (guide/) built at ${commit.slice(0, 9)}.` +
      (from > 0 ? ` These are my entries since my last export (${entries.length} of ${progress.notebook.length}).` : ""),
    "",
    "I own the fxdax repo and I'm learning its code with the guide; I'm an experienced systems programmer, new to this codebase. Below is what I did in it and what I asked. Use it to see where I am:",
    "",
    "1. Answer my questions and respond to my notes from the code itself: read the files and snippets named, cite code by exact snippet (path plus text), never by line number.",
    "2. Where I predicted wrong or my design differs from fxdax's, explain the mechanism I missed, and name the test, TLA+ mutation (specs/*/mutations) or mutant (guest/mutants) that shows it.",
    "3. Then tell me what I should look at next, and ask me one question that checks I can predict the code's behaviour in a case the guide didn't show.",
  ];

  const predictions = of("prediction");
  if (predictions.length) {
    const right = predictions.filter((p) => p.right).length;
    out.push("", `## Predictions: ${right} of ${predictions.length} right`, "");
    for (const p of predictions) {
      out.push(
        p.right
          ? `- Right, ${where(p)}: "${p.question}" I answered "${p.chose}".`
          : `- **Wrong**, ${where(p)}: "${p.question}" I answered "${p.chose}"; the answer is "${p.answer}".`,
      );
    }
  }

  const designs = of("design");
  if (designs.length) {
    out.push("", "## My designs, written before I saw fxdax's", "");
    for (const d of designs) out.push(`- ${where(d)}: ${d.prompt.trim()}`, "", quote(d.text), "");
  }

  const catches = of("catch");
  if (catches.length) {
    out.push("", "## Safety catches: which invariant breaks without a rule", "");
    for (const c of catches) {
      out.push(
        c.guessed === c.actual
          ? `- Without \`${c.rule}\` I said \`${c.guessed}\` breaks: right, TLC finds it violated.`
          : `- **Wrong**: without \`${c.rule}\` I said \`${c.guessed}\` breaks; TLC finds \`${c.actual}\` violated first.`,
      );
    }
  }

  const finds = of("find");
  if (finds.length) {
    out.push("", "## Finding the code without diagrams", "");
    for (const f of finds) {
      out.push(
        f.right
          ? `- "${f.task}": found \`${f.picked}\` after ${clock(f.ms)}.`
          : `- "${f.task}": picked \`${f.picked}\` at ${clock(f.ms)}, which is not it.`,
      );
    }
  }

  const marks = Object.entries(progress.marks);
  if (marks.length) {
    const by = (mark: Mark) => marks.filter(([, m]) => m === mark).map(([id]) => itemName(id));
    out.push("", "## What I say I know, as of now", "");
    for (const [mark, label] of [["know", "I know"], ["fuzzy", "I'm fuzzy on"], ["unknown", "I don't understand"]] as const) {
      const items = by(mark);
      if (items.length) out.push(`- ${label}: ${items.join("; ")}.`);
    }
  }

  const said = of("question", "note");
  if (said.length) {
    out.push("", "## My questions and notes, in order", "");
    said.forEach((s, i) => {
      out.push(`${i + 1}. **${s.kind === "question" ? "Question" : "Note"}** ${where(s)}:`);
      if (s.place.quote) out.push("", quote(s.place.quote).replaceAll("\n", "\n   ").replace(/^/, "   "));
      out.push("", `   ${s.text.trim().replaceAll("\n", "\n   ")}`, "");
    });
  }

  if (entries.length === 0) out.push("", "(Nothing new since my last export.)");
  return `${out.join("\n").replace(/\n{3,}/g, "\n\n").trim()}\n`;
}
