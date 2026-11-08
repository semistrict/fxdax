# fxdax Flight School

An interactive guide to this repo's code, built from the code itself, its design docs, its TLA+
specs, its past bugs and its dead ends. It has four parts:

- **Atlas.** The stack from the guest's Fxfs down to the bucket. You mark what you know, and it
  writes a prompt for Claude Code naming your gaps.
- **Invariants.** The claims that let you predict the code, each with the code that keeps it and
  the evidence that it is needed.
- **Index.** Every symbol of fxdax, and of the Fuchsia code it copies, labelled by origin.
- **Page Pilot.** A mission: follow one DAX page through copy-on-write, predicting before each
  reveal, then replay TLC's counterexamples with a rule removed.

Why it is built this way: [RESEARCH.md](RESEARCH.md).

## Run it

```
pnpm install
pnpm dev        # generates the data, then serves it at http://localhost:5173
pnpm build      # generates, checks, type-checks and builds dist/
pnpm test
```

Node 24 or later; the tools are TypeScript, run by Node directly.

## How it stays true

- **Code is cited by text, never by line.** `src/content/anchors.ts` names each place the content
  points to by file and exact snippet. `pnpm check`, which `pnpm build` runs, fails when a snippet
  no longer occurs exactly once in its file. It also fails when an invariant's evidence (a doc, a
  dead end, a mutant, a TLA+ mutation) is gone. CI's `guide` job runs the build.
- **Code is shown from the local source.** `tools/sources.ts` copies the files the guide shows
  into the build. A code card expands to show the code around its snippet, and gives the `rg` that
  finds it in a checkout. Nothing links to a website.
- **Generated, not written:**
  - `tools/index.ts`: the symbol index, from tree-sitter (web-tree-sitter with the Rust and C
    grammars). It classifies origin as `scripts/upstream-changes.py` and README's stand-in table
    do.
  - `tools/evidence.ts`: mutants, specs, mutations, dead ends and docs, read where they live.
  - Both write `src/generated/`, which is not committed.
- **TLC traces are recorded.** `pnpm traces [SPEC/NAME...]` runs TLC (the jar
  `scripts/check-tla.sh` pins) on the specs' mutations and writes `fixtures/traces/`, which is
  committed. Rerun it when a spec changes; `pnpm check` fails if a mission's trace is missing.

## Content

- `src/content/`: anchors, layers and invariants.
- `src/missions/pagePilot/`: the mission's steps (MDX), its predictions, scenes and transfer tasks.

A claim's standing is `checked` when a mutation or mutant shows it is needed, and `stated` when
only the docs and the code say it. The check holds every invariant to its label.
