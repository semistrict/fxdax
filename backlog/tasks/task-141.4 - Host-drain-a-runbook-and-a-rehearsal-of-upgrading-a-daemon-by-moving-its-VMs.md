---
id: TASK-141.4
title: >-
  Host drain: a runbook, and SIGTERM closing every store durably before the
  daemon exits
status: To Do
assignee: []
created_date: '2026-10-31 20:30'
updated_date: '2026-11-06 00:44'
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
- [ ] #1 A runbook covers drain, upgrade and check, and says what a format change refuses
- [ ] #2 scripts/migrate-gce.sh moves a VM off a host and back, both hosts of one build, nightly
- [ ] #3 SIGTERM's stores are durable and closed before the daemon exits, by test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: rewritten to fit AGENTS.md's no-migrations rule (75bd872ba): the runbook's rollback and the N-1/N rehearsal went.
<!-- SECTION:NOTES:END -->
