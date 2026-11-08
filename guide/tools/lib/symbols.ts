// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// fxdax's symbols, and those of the Fuchsia code it copies, with tree-sitter: each function, type,
// trait, module, const and static, its file, its signature line (an exact text anchor), the first
// sentence of its doc comment, what it is in (an impl, trait or module), and its file's origin.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { join, relative } from "node:path";
import { Language, type Node, Parser } from "web-tree-sitter";
import type { Kind, Origin } from "../../src/data/types.ts";
import { crateOrigins, fileOrigin, REPO } from "./repo.ts";

export interface Symbol {
  path: string;
  kind: Kind;
  name: string;
  /** The impl (`Type`, `Trait for Type`), trait or module it is in; "" at a file's top. */
  container: string;
  /** Its first line, without a trailing `{`: the text a link searches for. */
  signature: string;
  /** Whether `signature` occurs once in its file, so that a search for it finds this. */
  unique: boolean;
  /** The first sentence of its doc comment. */
  doc: string;
  origin: Origin;
}

/** fxdax's code outside crates/, every file of it fxdax's own. */
export const EXTRA_ROOTS = [
  "guest/rust/fxfs-linux/src",
  "guest/rust/linux-rt/src",
  "guest/linux/fs/fxfs",
  "guest/linux/drivers/virtio_extent",
  "vendor/firecracker/src/vmm/src/devices/virtio/extent",
];

const RUST_KINDS: Record<string, Kind> = {
  function_item: "fn",
  function_signature_item: "fn",
  struct_item: "struct",
  enum_item: "enum",
  trait_item: "trait",
  mod_item: "mod",
  const_item: "const",
  static_item: "static",
  type_item: "type",
};

const require = createRequire(import.meta.url);

export async function parsers(): Promise<{ rust: Parser; c: Parser }> {
  await Parser.init();
  const rust = new Parser();
  rust.setLanguage(await Language.load(require.resolve("tree-sitter-rust/tree-sitter-rust.wasm")));
  const c = new Parser();
  c.setLanguage(await Language.load(require.resolve("tree-sitter-c/tree-sitter-c.wasm")));
  return { rust, c };
}

/** Up to the first full stop that ends a sentence: not one in `a.b`, `e.g.` or `i.e.`. */
export function firstSentence(text: string): string {
  const flat = text.split(/\s+/).filter(Boolean).join(" ");
  const match = /^(.+?(?<!\be\.g)(?<!\bi\.e)\.)(?=\s+[A-Z(`*[]|$)/.exec(flat);
  const sentence = match ? match[1] : flat;
  return sentence.length <= 240 ? sentence : `${sentence.slice(0, 239)}…`;
}

/** The siblings right before `node` that are its comments and attributes, nearest first. */
function preceding(node: Node): Node[] {
  const out: Node[] = [];
  for (let prev = node.previousNamedSibling; prev; prev = prev.previousNamedSibling) {
    if (prev.type !== "line_comment" && prev.type !== "attribute_item") break;
    out.push(prev);
  }
  return out;
}

function rustDoc(node: Node): string {
  const lines: string[] = [];
  for (const prev of preceding(node)) {
    if (prev.type !== "line_comment") continue;
    if (!prev.text.startsWith("///")) break;
    lines.push(prev.text.slice(3).trim());
  }
  return firstSentence(lines.reverse().join(" "));
}

/** Whether `node` is test code: `#[test]`, `#[tokio::test]` and the like, or `#[cfg(test)]`. */
function isTest(node: Node): boolean {
  return preceding(node).some((prev) => {
    if (prev.type !== "attribute_item") return false;
    const text = prev.text;
    return text.includes("cfg(test)") || (/\btest\b/.test(text) && !text.includes("cfg"));
  });
}

export function signature(node: Node): string {
  return node.text.split("\n", 1)[0].replace(/[\s{]+$/, "").trim();
}

function walkRust(node: Node, path: string, container: string, out: Omit<Symbol, "unique" | "origin">[]) {
  for (const child of node.namedChildren) {
    if (child === null) continue;
    if (isTest(child)) continue;
    const kind = RUST_KINDS[child.type];
    const name = child.childForFieldName("name");
    if (kind && name) {
      out.push({ path, kind, name: name.text, container, signature: signature(child), doc: rustDoc(child) });
    }
    if (child.type === "impl_item") {
      const type = child.childForFieldName("type")?.text ?? "?";
      const trait = child.childForFieldName("trait")?.text;
      const body = child.childForFieldName("body");
      if (body) walkRust(body, path, trait ? `${trait} for ${type}` : type, out);
    } else if ((child.type === "mod_item" || child.type === "trait_item") && name) {
      const body = child.childForFieldName("body");
      if (body) walkRust(body, path, name.text, out);
    }
  }
}

function cDoc(node: Node): string {
  const prev = node.previousNamedSibling;
  if (!prev || prev.type !== "comment") return "";
  const text = prev.text
    .replace(/^\/\*+|\*+\/$/g, "")
    .replace(/\n\s*\* ?/g, " ")
    .replace(/^\/\/\s?/gm, "");
  return firstSentence(text);
}

function declaratorName(node: Node | null): Node | null {
  while (node && node.type !== "identifier" && node.type !== "field_identifier") {
    node = node.childForFieldName("declarator");
  }
  return node;
}

function walkC(root: Node, path: string, out: Omit<Symbol, "unique" | "origin">[]) {
  for (const child of root.namedChildren) {
    if (child === null) continue;
    if (child.type === "function_definition") {
      const name = declaratorName(child.childForFieldName("declarator"));
      if (name) {
        out.push({ path, kind: "fn", name: name.text, container: "", signature: signature(child), doc: cDoc(child) });
      }
    } else if (child.type === "struct_specifier" || child.type === "declaration") {
      const spec = child.type === "struct_specifier" ? child : child.childForFieldName("type");
      if (spec?.type === "struct_specifier" && spec.childForFieldName("body")) {
        const name = spec.childForFieldName("name");
        if (name) {
          out.push({ path, kind: "struct", name: name.text, container: "", signature: signature(child), doc: cDoc(child) });
        }
      }
    }
  }
}

function* files(dir: string): Generator<string> {
  for (const entry of readdirSync(dir).sort()) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) yield* files(full);
    else yield full;
  }
}

/** Whether a file is test or bench code, which the index leaves out. */
export function isTestPath(path: string): boolean {
  return (
    /\/(tests|benches|target)\//.test(path) ||
    path.endsWith("/tests.rs") ||
    path.endsWith("_tests.rs")
  );
}

/** Every symbol of the repo's code, by file, then by order in it. */
export async function indexRepo(repo = REPO): Promise<Symbol[]> {
  const { rust, c } = await parsers();
  const crates = crateOrigins(repo);
  const roots: [string, Origin][] = [
    ...[...crates].map(([name, origin]): [string, Origin] => [`crates/${name}`, origin]),
    ...EXTRA_ROOTS.map((root): [string, Origin] => [root, "fxdax"]),
  ];
  const symbols: Symbol[] = [];
  for (const [root, crateOrigin] of roots) {
    for (const file of files(join(repo, root))) {
      const path = relative(repo, file);
      if (isTestPath(path)) continue;
      const isRust = path.endsWith(".rs");
      if (!isRust && !path.endsWith(".c") && !path.endsWith(".h")) continue;
      const text = readFileSync(file, "utf8");
      const tree = (isRust ? rust : c).parse(text);
      if (!tree) throw new Error(`tree-sitter could not parse ${path}`);
      const found: Omit<Symbol, "unique" | "origin">[] = [];
      if (isRust) walkRust(tree.rootNode, path, "", found);
      else walkC(tree.rootNode, path, found);
      tree.delete();
      const origin = fileOrigin(crateOrigin, text);
      for (const sym of found) {
        symbols.push({ ...sym, origin, unique: occurrences(text, sym.signature) === 1 });
      }
    }
  }
  return symbols;
}

export function occurrences(text: string, snippet: string): number {
  if (!snippet) return 0;
  let count = 0;
  for (let at = text.indexOf(snippet); at !== -1; at = text.indexOf(snippet, at + 1)) count++;
  return count;
}
