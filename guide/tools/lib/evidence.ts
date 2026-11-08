// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The repo's evidence, read where it lives: the mutants of guest/mutants, the TLA+ specs and
// their mutations under specs/, the dead ends of dead-ends/, and the backlog's design docs.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import type { DeadEnd, Doc, Evidence, Mutant, Mutation, Spec } from "../../src/data/types.ts";
import { REPO } from "./repo.ts";

function sorted(dir: string, suffix: string): string[] {
  return readdirSync(dir)
    .filter((name) => name.endsWith(suffix))
    .sort();
}

/** A paragraph's lines joined, as prose: the source's line breaks are its wrapping. */
function prose(lines: string[]): string {
  return lines
    .join("\n")
    .split(/\n\s*\n/)
    .map((para) => para.split(/\s*\n\s*/).join(" ").trim())
    .filter(Boolean)
    .join("\n\n");
}

/** A mutant's patch: its `# Kernel:` and `# Killer:`, its story, and what it changes. */
export function parseMutant(file: string, text: string): Mutant {
  const header: string[] = [];
  let body = 0;
  const lines = text.split("\n");
  for (; body < lines.length && (lines[body].startsWith("#") || lines[body] === ""); body++) {
    header.push(lines[body]);
  }
  const field = (name: string) => {
    const line = header.find((l) => l.startsWith(`# ${name}:`));
    if (line === undefined) throw new Error(`guest/mutants/${file} has no ${name}:`);
    return line.slice(name.length + 3).trim();
  };
  const story = header
    .map((l) => l.replace(/^#\s?/, ""))
    .filter((l) => !/^(SPDX-License-Identifier|Copyright|Kernel:|Killer:)/.test(l));
  const patch = lines.slice(body).join("\n");
  const touches = [...patch.matchAll(/^\+\+\+ b\/(\S+)/gm)].map((m) => m[1]);
  return { file, kernel: field("Kernel"), killer: field("Killer"), story: prose(story), touches, patch };
}

/** A spec's AllRules, the rules its mutations each leave one of out of Rules. */
function allRules(tla: string): string[] {
  const match = /AllRules\s*==\s*\{([^}]*)\}/.exec(tla);
  return match ? [...match[1].matchAll(/"(\w+)"/g)].map((m) => m[1]) : [];
}

export function parseMutation(spec: string, name: string, cfg: string, rules: string[]): Mutation {
  const comment: string[] = [];
  let expect = "";
  for (const line of cfg.split("\n")) {
    if (!line.startsWith("\\*")) break;
    const text = line.replace(/^\\\*\s?/, "");
    if (text.startsWith("expect:")) expect = text.slice(7).trim();
    else comment.push(text);
  }
  if (!expect) throw new Error(`specs/${spec}/mutations/${name}.cfg names no expected violation`);
  const given = /Rules\s*=\s*\{([^}]*)\}/.exec(cfg);
  const kept = new Set(given ? [...given[1].matchAll(/"(\w+)"/g)].map((m) => m[1]) : rules);
  return { spec, name, story: prose(comment), expect, removed: rules.filter((r) => !kept.has(r)) };
}

/** The text of the spec's header comment, `(* ... *)` lines unboxed. */
function headerText(tla: string): string[] {
  const lines: string[] = [];
  let inside = false;
  for (const line of tla.split("\n")) {
    if (line.startsWith("(***")) {
      if (inside) break;
      inside = true;
      continue;
    }
    if (!inside) continue;
    if (line.startsWith("(*")) lines.push(line.replace(/^\(\*\s?/, "").replace(/\s*\*\)\s*$/, ""));
  }
  return lines;
}

export function parseSpec(spec: string, path: string, tla: string, checked: Set<string>): Spec {
  const header = headerText(tla).filter((l) => !/^(Copyright|Use of this|found in the)/.test(l));
  const rules: Record<string, string> = {};
  const summary: string[] = [];
  let rule: string | null = null;
  let afterRules = false;
  for (const line of header) {
    const bullet = /^- (\w+): (.*)$/.exec(line);
    if (bullet && allRules(tla).includes(bullet[1])) {
      rule = bullet[1];
      rules[rule] = bullet[2];
      continue;
    }
    if (rule && line.startsWith("  ")) {
      rules[rule] += ` ${line.trim()}`;
      continue;
    }
    if (rule && !line.trim()) {
      rule = null;
      afterRules = true;
      continue;
    }
    rule = null;
    if (!afterRules) summary.push(line);
  }
  const definitions: Record<string, string> = {};
  const invariants: Record<string, string> = {};
  const lines = tla.split("\n");
  lines.forEach((line, at) => {
    const def = /^([A-Z]\w*)(\([^)]*\))?\s*==/.exec(line);
    if (!def) return;
    const comment: string[] = [];
    for (let up = at - 1; up >= 0 && lines[up].startsWith("\\*"); up--) {
      comment.unshift(lines[up].replace(/^\\\*\s?/, ""));
    }
    if (comment.length) definitions[def[1]] = prose(comment);
    if (checked.has(def[1])) {
      const body: string[] = [line];
      for (let down = at + 1; down < lines.length && lines[down].trim() && !lines[down].startsWith("\\*"); down++) {
        body.push(lines[down]);
      }
      invariants[def[1]] = body.join("\n");
    }
  });
  return { spec, path, summary: prose(summary), rules, definitions, invariants };
}

export function parseDeadEnd(file: string, text: string): DeadEnd {
  const title = /^# (.+)$/m.exec(text)?.[1] ?? file;
  const fields: Record<string, string> = {};
  let label: string | null = null;
  for (const line of text.split("\n").slice(1)) {
    const start = /^\*\*([^*]+?):\*\*\s*(.*)$/.exec(line);
    if (start) {
      label = start[1].toLowerCase();
      fields[label] = start[2];
    } else if (label) fields[label] += `\n${line}`;
  }
  const get = (...names: string[]) => prose([names.map((n) => fields[n]).find(Boolean) ?? ""]);
  return {
    file,
    title,
    theory: file.startsWith("theory-") || /^Wrong theory/i.test(title),
    tried: get("tried"),
    idea: get("idea", "theory"),
    result: get("result"),
    why: get("why it's dead", "why it is dead", "why it was wrong", "why"),
  };
}

export function parseDoc(file: string, text: string): Doc {
  const id = /^id:\s*(\S+)/m.exec(text)?.[1];
  const title = /^title:\s*(.+)$/m.exec(text)?.[1];
  if (!id || !title) throw new Error(`backlog/docs/${file} has no id or title`);
  const headings = [...text.matchAll(/^#{1,3} (.+)$/gm)].map((m) => m[1].trim());
  // YAML's quoted scalars: '' is one ' inside single quotes.
  const unquoted = /^'(.*)'$/.test(title) ? title.slice(1, -1).replaceAll("''", "'") : title.replace(/^"(.*)"$/, "$1");
  return { id, title: unquoted, file, headings };
}

export function readEvidence(repo = REPO): Evidence {
  const mutantsDir = join(repo, "guest", "mutants");
  const mutants = sorted(mutantsDir, ".patch").map((f) =>
    parseMutant(f, readFileSync(join(mutantsDir, f), "utf8")),
  );
  const mutations: Mutation[] = [];
  const specs: Spec[] = [];
  for (const spec of readdirSync(join(repo, "specs")).sort()) {
    const dir = join(repo, "specs", spec);
    const tlaFile = sorted(dir, ".tla")[0];
    if (!tlaFile) continue;
    const tla = readFileSync(join(dir, tlaFile), "utf8");
    const rules = allRules(tla);
    const checked = new Set<string>();
    const cfgs = [
      ...sorted(dir, ".cfg").map((f) => join(dir, f)),
      ...sorted(join(dir, "mutations"), ".cfg").map((f) => join(dir, "mutations", f)),
    ];
    for (const cfg of cfgs) {
      for (const m of readFileSync(cfg, "utf8").matchAll(/^(?:INVARIANTS?|PROPERTY|PROPERTIES)\s+(.+)$/gm)) {
        for (const name of m[1].split(/\s+/)) checked.add(name);
      }
    }
    for (const cfg of sorted(join(dir, "mutations"), ".cfg")) {
      const name = cfg.replace(/\.cfg$/, "");
      mutations.push(parseMutation(spec, name, readFileSync(join(dir, "mutations", cfg), "utf8"), rules));
    }
    specs.push(parseSpec(spec, `specs/${spec}/${tlaFile}`, tla, checked));
  }
  const deadEndsDir = join(repo, "dead-ends");
  const deadEnds = sorted(deadEndsDir, ".md").map((f) =>
    parseDeadEnd(f, readFileSync(join(deadEndsDir, f), "utf8")),
  );
  const docsDir = join(repo, "backlog", "docs");
  const docs = sorted(docsDir, ".md")
    .map((f) => parseDoc(f, readFileSync(join(docsDir, f), "utf8")))
    .sort((a, b) => Number(a.id.split("-")[1]) - Number(b.id.split("-")[1]));
  return { mutants, mutations, specs, deadEnds, docs };
}
