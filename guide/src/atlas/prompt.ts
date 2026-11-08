// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { anchors } from "../content/anchors.ts";
import { invariants } from "../content/invariants.ts";
import { layers } from "../content/layers.ts";
import type { Mark } from "../data/progress.ts";

// The prompt the atlas writes for Claude Code in a checkout of this repo: what the reader marked
// fuzzy or unknown, with the files, anchors and docs to start from, and what they already know,
// to build on. It asks for explanations grounded in the code, and for questions back.

export function learningPrompt(marks: Record<string, Mark>): string | null {
  const of = (mark: Mark) => [
    ...layers.filter((l) => marks[`layer:${l.id}`] === mark).map((l) => ({ kind: "layer" as const, l })),
    ...invariants.filter((i) => marks[`inv:${i.id}`] === mark).map((i) => ({ kind: "inv" as const, i })),
  ];
  const fuzzy = of("fuzzy");
  const unknown = of("unknown");
  const known = of("know");
  if (!fuzzy.length && !unknown.length) return null;

  const describe = (item: (typeof fuzzy)[number]) => {
    if (item.kind === "layer") {
      const refs = item.l.anchors
        .slice(0, 4)
        .map((id) => `    - ${anchors[id].path}: \`${anchors[id].snippet}\``)
        .join("\n");
      return `- The ${item.l.name} layer (${item.l.what}). Directories: ${item.l.dirs.join(", ")}. Docs: ${item.l.docs.join(", ")}.\n  Start from:\n${refs}`;
    }
    const refs = item.i.enforcedBy.map((id) => `    - ${anchors[id].path}: \`${anchors[id].snippet}\``).join("\n");
    return `- The claim "${item.i.claim}"\n  Enforced at:\n${refs}`;
  };
  const name = (item: (typeof known)[number]) => (item.kind === "layer" ? `the ${item.l.name} layer` : `"${item.i.claim}"`);

  const parts = [
    "I own the fxdax repo (this checkout) and I'm learning its code. I'm an experienced systems programmer, new to this codebase.",
  ];
  if (unknown.length) parts.push(`I don't understand these yet:\n${unknown.map(describe).join("\n")}`);
  if (fuzzy.length) parts.push(`I'm fuzzy on these:\n${fuzzy.map(describe).join("\n")}`);
  if (known.length) parts.push(`I already understand ${known.map(name).join("; ")}. Build on that.`);
  parts.push(
    [
      "Teach me these from the code itself. For each:",
      "1. Read the files and anchors above (rg -n -F the snippet), and the docs they cite, before you answer.",
      "2. Before you explain, ask me to predict one behaviour, and wait for my answer.",
      "3. Then explain the mechanism and why it is so, citing code by exact snippet (path plus text), never by line number.",
      "4. Name the test, TLA+ mutation (specs/*/mutations) or mutant (guest/mutants) that would fail if it weren't so, if one exists.",
      "5. End with one question that checks I can predict the code's behaviour in a case you didn't show me.",
    ].join("\n"),
  );
  return parts.join("\n\n");
}
