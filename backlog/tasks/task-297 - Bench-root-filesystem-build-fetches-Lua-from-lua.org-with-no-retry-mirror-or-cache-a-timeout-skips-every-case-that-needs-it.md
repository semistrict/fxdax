---
id: TASK-297
title: >-
  Bench root filesystem build fetches Lua from lua.org with no retry, mirror or
  cache: a timeout skips every case that needs it
status: To Do
assignee: []
created_date: '2026-11-09 09:38'
labels:
  - bench
  - infra
  - flaky
dependencies: []
priority: medium
ordinal: 706000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
test_fs_bench.rs builds the bench guests' Alpine root filesystem with podman, its Containerfile running `wget -q -O /opt/lua-5.4.7.tar.gz https://www.lua.org/ftp/lua-5.4.7.tar.gz` (checked by sha256) once per build, no retry, no mirror, nothing cached on the host. When lua.org is slow or unreachable the build fails and every case on that root filesystem SKIPs (`podman build failed: wget: download timed out` / `can't connect to remote host (46.175.8.47)`), and the run reports FAIL with nothing measured. Hit twice: restructure-before-after r910 (2026-11-07, bench-fork-fxfs-guest-dax-warm skipped, rerun as r912) and TASK-275 (2026-11-09, n2 us-east1-b: five of six bench-fork cases on 557b49a70 skipped, docs/measurements/perf-suite-pci-vs-before-2026-11-08). A measurement campaign should not depend on one external host being up.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The Lua tarball comes from a host-side cache (or the repo / a bucket we control), fetched at most once per host and verified by its sha256
- [ ] #2 A download that fails is retried, with a second source, before the build fails
- [ ] #3 bench-fs and bench-fork cases build their root filesystem with lua.org unreachable (tested by blocking it)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
