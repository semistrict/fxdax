---
id: decision-1
title: >-
  libkrun changes live on semistrict/libkrun branch fxdax, vendored as a
  submodule
date: '2026-09-28 02:49'
status: accepted
---
## Context

fxdax needs changes to libkrun: an `FsBackend` variant with a virtio-fs adapter, and DAX fault routing on HVF and KVM. semistrict/libkrun's `redoxfs` branch already generalises the fs device over `FsBackend` and carries a working DAX backend (see doc-1).

## Decision

libkrun changes for fxdax go on branch `fxdax` of semistrict/libkrun, branched from `redoxfs` (8cf90ff). The branch is vendored in this repo as the submodule `vendor/libkrun`, which tracks `fxdax`.

## Consequences

- fxdax reuses the RedoxFS backend's plumbing (`FsBackend`, the macOS VMM worker start for DAX windows, and the e2e test harness).
- The submodule commit must be pushed to semistrict/libkrun before a fresh clone of fxdax can check it out. Pushing needs explicit approval.
- Changes that also belong to redoxfs (for example, fault routing for DAX windows) may later be split out to `redoxfs` or `main`.

