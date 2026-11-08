// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import { useEffect, useState } from "react";
import type { Fork, Scene } from "./scene.ts";

// Two forks' windows above the daemon's physical memory, and the extents below: which page each
// window maps, and whose. Illustration of doc-13's protocol, one page wide.

const W = 760;
const H = 330;
const FORK_W = 340;
const PAGE = { dx: 14, dy: 40, w: 160, h: 58 };
const HOST_Y = 160;
const SLOT = { y: HOST_Y + 14, w: 150, h: 58 };
const PRIVATE_X = 570;

function cacheX(i: number) {
  return 24 + i * 172;
}

function pageLabel(fork: Fork): [string, string] {
  switch (fork.page.kind) {
    case "extent":
      return [`maps ${fork.page.extent}`, fork.page.data];
    case "zeros":
      return ["maps zeros", "0"];
    case "private":
      return ["private page", fork.page.data];
  }
}

function ForkBox({ fork, x, scene, dim, isA }: { fork: Fork; x: number; scene: Scene; dim: boolean; isA: boolean }) {
  const [kind, data] = pageLabel(fork);
  const private_ = fork.page.kind === "private";
  return (
    <g opacity={dim ? 0.42 : 1}>
      <rect x={x} y={10} width={FORK_W} height={124} rx={8} fill="var(--panel)" stroke="var(--l-guest)" strokeWidth={1.5} />
      <text x={x + 14} y={32} fontFamily="var(--display)" fontWeight={700} fontSize={15} fill="currentColor">
        {fork.name}
      </text>
      <text x={x + FORK_W - 14} y={32} textAnchor="end" fontFamily="var(--mono)" fontSize={11} fill="var(--ink-3)">
        window page 0
      </text>
      <rect
        x={x + PAGE.dx}
        y={PAGE.dy + 10}
        width={PAGE.w}
        height={PAGE.h}
        rx={5}
        fill={private_ ? "color-mix(in srgb, var(--l-memory) 18%, var(--panel))" : "var(--sunk)"}
        stroke={private_ ? "var(--l-memory)" : "var(--rule)"}
        strokeWidth={private_ ? 2 : 1}
      />
      <text x={x + PAGE.dx + 10} y={PAGE.dy + 32} fontFamily="var(--mono)" fontSize={12} fill="var(--ink-2)">
        {kind}
      </text>
      <text x={x + PAGE.dx + 10} y={PAGE.dy + 54} fontFamily="var(--mono)" fontWeight={700} fontSize={16} fill="currentColor">
        {data}
      </text>
      <text x={x + 192} y={PAGE.dy + 26} fontFamily="var(--mono)" fontSize={12} fill="var(--ink-2)">
        PTE: {fork.pte === "none" ? "none" : fork.pte === "ro" ? "read-only" : "writable"}
      </text>
      <text x={x + 192} y={PAGE.dy + 46} fontFamily="var(--mono)" fontSize={12} fill="var(--ink-2)">
        sets: {fork.sets.length ? fork.sets.join(", ") : "none"}
      </text>
      {isA && (
        <text
          x={x + 192}
          y={PAGE.dy + 66}
          fontFamily="var(--mono)"
          fontSize={12}
          fill={scene.semaphore === "exclusive" ? "var(--l-vmm)" : "var(--ink-2)"}
          fontWeight={scene.semaphore === "exclusive" ? 700 : 400}
        >
          semaphore: {scene.semaphore}
        </text>
      )}
    </g>
  );
}

export function Machine({ scene }: { scene: Scene }) {
  const cached = scene.extents.filter((e) => e.cached);
  const dim = (part: Scene["focus"][number]) => scene.focus.length > 0 && !scene.focus.includes(part);
  const target = (fork: Fork): { x: number; y: number } | null => {
    if (fork.page.kind === "private") return { x: PRIVATE_X + 83, y: SLOT.y };
    if (fork.page.kind === "extent") {
      const i = cached.findIndex((e) => e.id === fork.page.extent);
      return i >= 0 ? { x: cacheX(i) + SLOT.w / 2, y: SLOT.y } : null;
    }
    return null;
  };
  const arrow = (fork: Fork, x: number, key: string) => {
    const to = target(fork);
    if (!to) return null;
    const from = { x: x + PAGE.dx + PAGE.w / 2, y: PAGE.dy + 10 + PAGE.h };
    const mid = (from.y + to.y) / 2;
    return (
      <path
        key={key}
        d={`M ${from.x} ${from.y} C ${from.x} ${mid}, ${to.x} ${mid}, ${to.x} ${to.y - 4}`}
        fill="none"
        stroke={fork.page.kind === "private" ? "var(--l-memory)" : "var(--l-daemon)"}
        strokeWidth={2}
        markerEnd="url(#pp-arrow)"
      />
    );
  };
  const privateA = scene.a.page.kind === "private" ? scene.a.page.data : null;
  return (
    <svg viewBox={`0 0 ${W} ${H}`} role="img" aria-label={scene.caption}>
      <defs>
        <marker id="pp-arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
          <path d="M 0 0 L 10 5 L 0 10 z" fill="context-stroke" />
        </marker>
      </defs>
      <ForkBox fork={scene.a} x={10} scene={scene} dim={dim("a")} isA />
      <ForkBox fork={scene.b} x={W - 10 - FORK_W} scene={scene} dim={dim("b")} isA={false} />

      <rect x={10} y={HOST_Y} width={W - 20} height={100} rx={8} fill="none" stroke="var(--l-memory)" strokeDasharray="5 4" />
      <text x={24} y={HOST_Y + 90} fontFamily="var(--display)" fontWeight={700} fontSize={13} fill="var(--l-memory)">
        extentd’s physical memory (memfds the VM processes map)
      </text>
      <g opacity={dim("cache") ? 0.42 : 1}>
        {cached.map((e, i) => (
          <g key={e.id}>
            <rect x={cacheX(i)} y={SLOT.y} width={SLOT.w} height={SLOT.h} rx={5} fill="var(--sunk)" stroke="var(--l-daemon)" />
            <text x={cacheX(i) + 10} y={SLOT.y + 22} fontFamily="var(--mono)" fontSize={12} fill="var(--ink-2)">
              cache: {e.id} p0
            </text>
            <text x={cacheX(i) + 10} y={SLOT.y + 44} fontFamily="var(--mono)" fontWeight={700} fontSize={16} fill="currentColor">
              {e.data}
            </text>
          </g>
        ))}
      </g>
      <g opacity={dim("private") ? 0.42 : 1}>
        <rect
          x={PRIVATE_X}
          y={SLOT.y}
          width={166}
          height={SLOT.h}
          rx={5}
          fill={privateA ? "color-mix(in srgb, var(--l-memory) 18%, var(--panel))" : "none"}
          stroke="var(--l-memory)"
          strokeDasharray={privateA ? undefined : "3 3"}
        />
        <text x={PRIVATE_X + 10} y={SLOT.y + 22} fontFamily="var(--mono)" fontSize={12} fill="var(--ink-2)">
          A’s private pages
        </text>
        <text x={PRIVATE_X + 10} y={SLOT.y + 44} fontFamily="var(--mono)" fontWeight={700} fontSize={16} fill="currentColor">
          {privateA ?? "none"}
        </text>
      </g>
      {arrow(scene.a, 10, "a")}
      {arrow(scene.b, W - 10 - FORK_W, "b")}
      {scene.request && (
        <g>
          <rect x={W / 2 - 52} y={HOST_Y - 24} width={104} height={22} rx={11} fill="var(--accent)" />
          <text x={W / 2} y={HOST_Y - 9} textAnchor="middle" fontFamily="var(--mono)" fontWeight={700} fontSize={12} fill="var(--accent-ink)">
            {scene.request}
          </text>
        </g>
      )}
      <g opacity={dim("extents") ? 0.42 : 1}>
        <text x={14} y={HOST_Y + 136} fontFamily="var(--display)" fontWeight={700} fontSize={13} fill="var(--l-disk)">
          Extents (the store):
        </text>
        {scene.extents.map((e, i) => (
          <g key={e.id}>
            <rect
              x={150 + i * 196}
              y={HOST_Y + 120}
              width={184}
              height={26}
              rx={13}
              fill={e.state === "open" ? "none" : "color-mix(in srgb, var(--l-disk) 16%, var(--panel))"}
              stroke="var(--l-disk)"
              strokeDasharray={e.state === "open" ? "4 3" : undefined}
            />
            <text x={150 + i * 196 + 92} y={HOST_Y + 138} textAnchor="middle" fontFamily="var(--mono)" fontSize={12} fill="currentColor">
              {e.id} {e.state} · {e.data}
            </text>
          </g>
        ))}
      </g>
    </svg>
  );
}

/** A run of scenes, stepped through. */
export function SceneStepper({ scenes }: { scenes: Scene[] }) {
  const [at, setAt] = useState(0);
  useEffect(() => setAt(0), [scenes]);
  const scene = scenes[Math.min(at, scenes.length - 1)];
  return (
    <figure className="machine" style={{ margin: 0 }}>
      <div className="machine-scroll">
        <Machine scene={scene} />
      </div>
      <figcaption className="machine-caption">{scene.caption}</figcaption>
      {scenes.length > 1 && (
        <div className="machine-controls">
          <span className="eyebrow num">
            {at + 1} of {scenes.length} · illustration of doc-13
          </span>
          <div className="row">
            <button type="button" className="btn" disabled={at === 0} onClick={() => setAt(at - 1)}>
              Back
            </button>
            <button type="button" className="btn primary" disabled={at === scenes.length - 1} onClick={() => setAt(at + 1)}>
              Next
            </button>
          </div>
        </div>
      )}
    </figure>
  );
}
