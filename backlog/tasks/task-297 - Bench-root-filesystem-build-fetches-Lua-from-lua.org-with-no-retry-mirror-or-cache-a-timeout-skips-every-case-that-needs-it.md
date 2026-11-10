---
id: TASK-297
title: >-
  Bench root filesystem build fetches Lua from lua.org with no retry, mirror or
  cache: a timeout skips every case that needs it
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 09:38'
updated_date: '2026-11-10 09:30'
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
- [x] #1 The Lua tarball comes from a host-side cache (or the repo / a bucket we control), fetched at most once per host and verified by its sha256
- [x] #2 A download that fails is retried, with a second source, before the build fails
- [x] #3 bench-fs and bench-fork cases build their root filesystem with lua.org unreachable (tested by blocking it)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. test_cases::rootfs: Image { containerfile, downloads } (Test::rootfs_image returns it), Download { name, sha256, sources }: fetched into the host's cache (FXDAX_DOWNLOADS, else $XDG_CACHE_HOME or ~/.cache /fxdax/downloads) once, checked by sha256 when fetched and when used, curl retried (--retry 2 --retry-all-errors), sources in turn, renamed into place from a per-process partial. podman build's context a temp directory holding the downloads, not the runner's cwd; the tag hashes the downloads too.
2. test_fs_bench: COPY lua-5.4.7.tar.gz from lua.org, PUC-Rio's tecgraf and Buildroot's sources mirror (all checked: same sha256).
3. Tests: unit tests of fetch (unreachable first source passed over, cached copy used with every source unreachable, wrong bytes passed over); on ubu1 a bench-fs case's rootfs built with lua.org blocked.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Sources checked by hand (same sha256 9fbf5e28...): lua.org, tecgraf.puc-rio.br/lua/ftp, sources.buildroot.net; tecgraf's /mirror/ path and distfiles.gentoo/alpine 404. Unit tests (test_cases --features host, rootfs::): 3 pass on ubu1. E2E on ubu1 as ramon with prebuilt artifacts, FXDAX_BENCH_ITERATIONS=1 FXDAX_BENCH_GROUPS=build: warm run fetched from lua.org into an empty cache, built Lua (6.5 s); then with https_proxy a dead port (curl to lua.org: exit 7) and only the mirrors exempt, a fresh empty cache: bench-fs-fxfs-guest-dax built its root filesystem, the tarball fetched from a mirror (sha256 checked), Lua built (6.2 s). Blind spot: the build fetched inside the container, where nothing could retry or cache; the Image/Download type now makes a download a host-side, checked, cached input for any image.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Root filesystem images take downloads from a host cache (sha256-checked, fetched once, curl retried, several sources); Lua from lua.org, PUC-Rio or Buildroot. Verified with lua.org unreachable on ubu1.
<!-- SECTION:FINAL_SUMMARY:END -->
