// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import type { ComponentType } from "react";
import { setHere } from "../../data/place.ts";
import { MissionContext } from "../../ui/mdx.tsx";
import Shared from "./steps/1-shared.mdx";
import Store from "./steps/2-store.mdx";
import Fsync from "./steps/3-fsync.mdx";
import Catch from "./steps/4-catch.mdx";
import Road from "./steps/5-road.mdx";
import Transfer from "./steps/6-transfer.mdx";

interface Step {
  title: string;
  kicker: string;
  Body: ComponentType;
}

export const steps: Step[] = [
  { title: "One page, two forks", kicker: "Read", Body: Shared },
  { title: "A store in fork A", kicker: "Copy on write", Body: Store },
  { title: "fsync", kicker: "Writeback, seal, rebind", Body: Fsync },
  { title: "Pull a safety catch", kicker: "TLA+ counterexamples", Body: Catch },
  { title: "The road not taken", kicker: "Writable DAX", Body: Road },
  { title: "Find it yourself", kicker: "No diagrams", Body: Transfer },
];

export function PagePilot({ step }: { step: number }) {
  const at = Math.min(Math.max(step, 0), steps.length - 1);
  const { Body, title, kicker } = steps[at];
  setHere(`Page Pilot, step ${at + 1}: ${title}`, `pilot-${at + 1}`);
  return (
    <MissionContext.Provider value="page-pilot">
      <div className="lede">
        <span className="eyebrow">Mission · about 10 minutes</span>
        <h1>Page Pilot</h1>
        <p>
          Follow one page of one file through copy-on-write DAX: shared by two forks, made private by a store, written
          back, sealed and shared again. Predict before each reveal. The diagrams are illustrations of doc-13; the
          traces are TLC’s.
        </p>
      </div>
      <div className="mission">
        <nav className="steps" aria-label="Steps">
          {steps.map((s, i) => (
            <a key={s.title} href={`#pilot-${i + 1}`} aria-current={i === at ? "step" : undefined}>
              <span className="eyebrow">
                {i + 1} · {s.kicker}
              </span>
              <b>{s.title}</b>
            </a>
          ))}
        </nav>
        <article className="prose">
          <span className="eyebrow">
            Step {at + 1} · {kicker}
          </span>
          <h2>{title}</h2>
          <Body />
          <div className="step-nav">
            {at > 0 ? (
              <a className="btn" href={`#pilot-${at}`}>
                Back: {steps[at - 1].title}
              </a>
            ) : (
              <span />
            )}
            {at < steps.length - 1 ? (
              <a className="btn primary" href={`#pilot-${at + 2}`}>
                Next: {steps[at + 1].title}
              </a>
            ) : (
              <a className="btn primary" href="#atlas">
                Back to the atlas
              </a>
            )}
          </div>
        </article>
      </div>
    </MissionContext.Provider>
  );
}
