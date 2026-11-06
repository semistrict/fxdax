---
id: TASK-141.4
title: >-
  Host drain: a runbook, and SIGTERM closing every store durably before the
  daemon exits
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-31 20:30'
updated_date: '2026-11-06 04:47'
labels: []
dependencies: []
parent_task_id: TASK-141
ordinal: 491000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A daemon holds its VMs' memory, stores and connections, so a restart ends every VM on the host (158, ExtentdLost): upgrading it is a host drain (doc-11). Under AGENTS.md's no-migrations rule (2026-11-05) a drain moves VMs only to hosts of the same formats, or stops them, and a rollback past a format change is refused, not supported: no N-1 to N moves, no rollback rehearsal.

SIGTERM today makes the health service say NOT_SERVING and exits (crates/fxdax/src/bin/fxdax-extentd.rs:112-145, crates/fxdax/src/extentd/mod.rs:125-131): no store is closed, nor its uploads waited for.

Rewritten 2026-11-05: its runbook's rollback and the N-1/N rehearsal went with AGENTS.md's rule.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A runbook covers drain, upgrade and check, and says what a format change refuses
- [ ] #2 scripts/migrate-gce.sh moves a VM off a host and back, both hosts of one build, nightly
- [x] #3 SIGTERM's stores are durable and closed before the daemon exits, by test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extentd::drain: SIGTERM's thread drains rather than exits: health NOT_SERVING (as before); then, draining, the VM sockets' listeners (default and tenants') shut down, the control API stops taking calls and finishes those in flight, every connection is cut (its VM sees ExtentdLost, its requests served, commits made durable, store closed as a connection going closes it), points kept for forks closed; it waits for every store's uploader to have uploaded and stopped. Within FXDAX_EXTENTD_DRAIN seconds (default 60): exit 0, or past it, log what is still open and exit 1.
2. Connections counted in a watch (awaitable), listeners registered as they accept, the control server's graceful shutdown, a count of live uploaders (remote::Remote).
3. Host test (crates/fxdax/tests/drain.rs, Linux): a daemon with a file:// bucket, a VM process's commit, SIGTERM: it exits 0, the VM process told the daemon went, new connections refused, and a second daemon (later placement) takes the branch from the bucket with the commit: red before (exit without uploading).
4. Runbook docs/upgrading-a-host.md (drain, upgrade, check; what a format change refuses), linked from doc-11 and the integrator guide; doc-11 updated.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: rewritten to fit AGENTS.md's no-migrations rule (75bd872ba): the runbook's rollback and the N-1/N rehearsal went.

2026-11-05 implemented (f9fa1c94a): extentd::drain on SIGTERM (see plan); runbook docs/upgrading-a-host.md, linked from doc-11 and docs/integrator-guide.md.
- Bugs found: (1) the daemon's main thread returned once the drain shut its listener, exiting the process mid-drain: fixed, main parks. (2) A store closed leaves its open segment's commits unuploaded (ExtentStore::drop closes before its Remote stops; a closed store closes no segment), against Remote's docs: the drain uploads each connection's commits with the store still open (stores::uploaded) instead; dropping the Remote first broke TASK-92.5's take-back flows and serves_hundreds_of_forks_of_a_base, so it is TASK-192. Blind spot: every upload test either waited for the interval or made durable commits (which wait for upload); none closed a store right after an ordered commit. The drain host test is that test now.
- Red before: sigterm_closes_every_store_durable_and_uploaded failed with the exit at once (local root empty) and with the drain but no upload (bucket lacked the commit: only segment 0).
- AC #2 (migrate-gce.sh off and back nightly) not done: needs the nightly's GCP credentials and a bucket (OPEN_QUESTIONS).

Verified: sigterm_closes_every_store_durable_and_uploaded passes on Linux Docker and GCE r692 (x86); drain unit tests pass both. Left In Progress for AC #2 alone.
<!-- SECTION:NOTES:END -->
