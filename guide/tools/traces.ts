// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Records TLC's counterexamples of the specs' mutations as fixtures/traces/SPEC/NAME.json, the
// traces the guide replays. TLC runs as scripts/check-tla.sh runs it: its pinned jar (which that
// script fetches into ~/.cache/fxdax/tla), checked against its SHA-256, on $JAVA, java, or
// Homebrew's openjdk. Each mutation must violate the invariant its configuration expects.
//
// Usage: node tools/traces.ts [SPEC/NAME...]   (by default every mutation of specs/dax-cow)

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import type { Trace } from "../src/data/types.ts";
import { parseMutation } from "./lib/evidence.ts";
import { GUIDE, git, REPO } from "./lib/repo.ts";

const script = readFileSync(join(REPO, "scripts", "check-tla.sh"), "utf8");
const version = /^tla_version=(\S+)/m.exec(script)?.[1];
const sha256 = /^tla_sha256=(\S+)/m.exec(script)?.[1];
if (!version || !sha256) throw new Error("scripts/check-tla.sh pins no TLA+ release");
const jar = join(homedir(), ".cache", "fxdax", "tla", `tla2tools-${version}.jar`);
if (!existsSync(jar)) {
  throw new Error(`${jar} is missing: run scripts/check-tla.sh once, which fetches it`);
}
if (createHash("sha256").update(readFileSync(jar)).digest("hex") !== sha256) {
  throw new Error(`${jar} does not match the SHA-256 scripts/check-tla.sh pins`);
}

function java(): string {
  for (const candidate of [process.env.JAVA, "java", "/opt/homebrew/opt/openjdk/bin/java", "/usr/local/opt/openjdk/bin/java"]) {
    if (candidate && spawnSync(candidate, ["-version"]).status === 0) return candidate;
  }
  throw new Error("No Java runs: set JAVA");
}

const chosen =
  process.argv.length > 2
    ? process.argv.slice(2)
    : readdirSync(join(REPO, "specs", "dax-cow", "mutations"))
        .filter((f) => f.endsWith(".cfg"))
        .sort()
        .map((f) => `dax-cow/${f.replace(/\.cfg$/, "")}`);

const jvm = java();
const commit = git(["rev-parse", "--short", "HEAD"]);
const work = mkdtempSync(join(tmpdir(), "fxdax-traces-"));
let failed = 0;
try {
  for (const model of chosen) {
    const [spec, name] = model.split("/");
    const dir = join(REPO, "specs", spec);
    const tla = readdirSync(dir).find((f) => f.endsWith(".tla"));
    const cfg = join(dir, "mutations", `${name}.cfg`);
    if (!tla || !existsSync(cfg)) throw new Error(`no mutation ${model}`);
    const rules = [...(/AllRules\s*==\s*\{([^}]*)\}/.exec(readFileSync(join(dir, tla), "utf8"))?.[1] ?? "").matchAll(/"(\w+)"/g)].map((m) => m[1]);
    const mutation = parseMutation(spec, name, readFileSync(cfg, "utf8"), rules);
    const dump = join(work, `${spec}-${name}.json`);
    // TLC exits nonzero when it finds the violation, which is what a mutation must do.
    const run = spawnSync(
      jvm,
      ["-XX:+UseParallelGC", "-cp", jar, "tlc2.TLC", "-workers", "auto", "-noGenerateSpecTE",
       "-metadir", join(work, `${spec}-${name}.states`), "-dumpTrace", "json", dump,
       "-config", cfg, join(dir, tla)],
      { cwd: dir, encoding: "utf8", maxBuffer: 1 << 28 },
    );
    const violated = /^Error: Invariant (\w+) is violated\./m.exec(run.stdout)?.[1];
    if (violated !== mutation.expect || !existsSync(dump)) {
      console.error(`FAIL ${model}: expected ${mutation.expect} violated, TLC said:`);
      console.error(run.stdout.split("\n").filter((l) => l.startsWith("Error")).join("\n") || run.stderr);
      failed++;
      continue;
    }
    const raw = JSON.parse(readFileSync(dump, "utf8")) as {
      vars: string[];
      counterexample: { action: [[number, Record<string, unknown>], { name: string }, [number, Record<string, unknown>]][] };
    };
    const steps = raw.counterexample.action;
    const trace: Trace = {
      spec,
      mutation: name,
      violated,
      tlc: version,
      commit,
      vars: raw.vars,
      states: [
        { action: "Init", values: steps[0][0][1] },
        ...steps.map(([, action, [, after]]) => ({ action: action.name, values: after })),
      ],
    };
    const outDir = join(GUIDE, "fixtures", "traces", spec);
    mkdirSync(outDir, { recursive: true });
    writeFileSync(join(outDir, `${name}.json`), `${JSON.stringify(trace, null, 1)}\n`);
    console.log(`${model}: ${violated} violated after ${trace.states.length - 1} steps`);
  }
} finally {
  rmSync(work, { recursive: true, force: true });
}
process.exit(failed ? 1 : 0);
