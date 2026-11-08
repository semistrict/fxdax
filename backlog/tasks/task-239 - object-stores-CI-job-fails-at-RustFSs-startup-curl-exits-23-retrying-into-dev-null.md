---
id: TASK-239
title: >-
  object-stores CI job fails at RustFS's startup: curl exits 23 retrying into
  /dev/null
status: Done
assignee:
  - '@claude'
created_date: '2026-11-08 00:03'
updated_date: '2026-11-08 01:09'
labels:
  - testing
  - ci
dependencies: []
priority: medium
type: bug
ordinal: 674000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
CI run 42423429301 (main bc825b923): the object-stores job failed in scripts/test-rustfs.sh with exit 23, 10 s after RustFS 1.0.4's image was pulled, nothing said (curl -s). 1 failure in 48 completed object-stores runs since the job came (TASK-149.20).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Which curl failed, and why, reproduced
- [x] #2 The servers' startup waits on a real readiness signal: no probe retrying errors, no added sleep
- [x] #3 A test of the wait, and both scripts pass locally and in CI
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read the job's log; reproduce on Linux (ubuntu:24.04's curl 8.5.0, the runner's).
2. Readiness as the server says it: each container a health check run inside it (RustFS's /health/ready, Azurite's port listened), the script waiting for the engine's healthy event (docker events from before the run; podman wait --condition=healthy).
3. scripts/tests: a fake engine; the wait's healthy and never-healthy paths; the scripts probe nothing.
4. Both scripts locally, 10 starts, push land-h8 with CI.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Root cause: the readiness probe TASK-149.20 added, curl -s --retry 60 --retry-all-errors --retry-delay 1 -o /dev/null $AWS_ENDPOINT/. A probe from the host goes through the engine's port forwarding, which accepts before RustFS does and cuts transfers short; with --retry-all-errors curl retried one it had written part of, and to retry it truncates its output first: ftruncate of /dev/null fails on Linux (EINVAL), and curl gives up with CURLE_WRITE_ERROR, 23, silent under -s. Reproduced in ubuntu:24.04 (curl 8.5.0) against a server answering part of a body and closing: exit 23, with -sS 'curl: (18) transfer closed with 93 bytes remaining to read' first; macOS's curl 8.7.1 truncates /dev/null and goes on (exit 7 there), so it never showed locally. test-azurite.sh had the same probe. History: 1 failure in 48 completed object-stores runs (run 42423429301).
Fix (scripts/lib/containers.sh healthy_container): the container gets a health check run inside it, the server's own readiness (RustFS: curl -fsS of /health/ready, 200 once ready; Azurite: nc -z of its port, which it listens on once started); the script blocks on the engine's health_status: healthy event (docker events since before the run, through a FIFO, as macOS's bash 3.2 has no coproc; podman wait --condition=healthy), at most HEALTHY_WITHIN s (120), then the container's log and a stop. No probe from the host, no retries, no sleeps of the script's: the engine runs the check each 1 s. test-rustfs.sh runs RustFS in a container alone now (FXDAX_RUSTFS's binary, which had no health check, gone); the bucket and container makings are curl -fsS, saying why they fail.
Verified: test-rustfs.sh (4 of 4) and test-azurite.sh (1 of 1) locally on OrbStack's docker 29.8.2; 10 consecutive RustFS starts healthy in 1-2 s, each then making its bucket; a never-healthy container (health check false, HEALTHY_WITHIN=4) fails with its log, stopped; scripts/tests' test_containers.py (a fake engine: healthy after unhealthy, never healthy within 1 s, the scripts probe nothing) passes, and every script test (macOS). The podman path isn't run here (podman isn't running).

Landed: main 085a11d19, in 24639d64c (land-c5), CI 42429850075 green: object-stores (RustFS 4 of 4, Azurite 1 of 1 through healthy_container), test (clippy scripts) with test_containers.py, test (workspace), macos (workspace) and macos (shuttle). land-h8's own CI 42428413653: object-stores green. DoD #1 by CI's Linux and macOS workspace jobs, not GCE; #2 upstream-changes.py --check passes on main; #3 nothing of upstream's changed (scripts alone).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
CI's object-stores job failed once in 48 (run 42423429301) at RustFS's start: its readiness probe, curl retrying all errors into /dev/null through the engine's port forwarding, which cut a transfer short, had to truncate what it had written, which /dev/null refuses on Linux, and exited 23 (reproduced with ubuntu:24.04's curl 8.5.0; macOS's curl truncates it). The servers now start ready as their own health checks say, run inside each container (RustFS's /health/ready, Azurite's port listened), the scripts waiting for the engine's healthy event (scripts/lib/containers.sh healthy_container), with no probe from outside, no retries and no sleeps of their own. Verified: both scripts locally and 10 RustFS starts healthy in 1-2 s; test_containers.py with a fake engine; CI 42429850075 green on main.
<!-- SECTION:FINAL_SUMMARY:END -->
