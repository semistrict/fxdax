---
id: decision-1
title: >-
  libkrun changes live on semistrict/libkrun branch fxdax, vendored as a
  submodule
date: '2026-09-28 02:49'
status: accepted
---
## Context

fxdax needs changes to libkrun: a virtio-fs backend (`FsBackend`) with an adapter, and DAX fault routing on HVF and KVM (see doc-1).

## Decision

libkrun changes for fxdax go on branch `fxdax` of semistrict/libkrun, based on its `main`. The branch is vendored in this repo as the submodule `vendor/libkrun`, which tracks `fxdax`.

## Consequences

- The branch carries the generic plumbing fxdax needs as well: the fs device generalised over `FsBackend`, the macOS VMM worker started for DAX windows, and the e2e test harness.
- The submodule commit must be pushed to semistrict/libkrun before a fresh clone of fxdax can check it out. Pushing needs explicit approval.
- Generic changes (for example, fault routing for DAX windows) may later be split out to `main`.
