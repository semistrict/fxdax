---
id: TASK-224
title: >-
  An option for fsync to wait for local durability: a guest's fsync, fdatasync,
  msync and O_SYNC acknowledged only once the commit is synced to the host's
  disk, not just ordered in extentd's memory
status: To Do
assignee: []
created_date: '2026-11-07 04:40'
updated_date: '2026-11-07 04:40'
labels: []
dependencies:
  - TASK-165.9
priority: medium
type: feature
ordinal: 657000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Today (doc-12) a guest's fsync waits only for *ordered*: the commit is in extentd's memory and acknowledged at once, and becomes durable locally later (a 50 ms median, a 500 ms crash bound since TASK-221). So an extentd crash, or a host reboot with its disk kept, loses acknowledged fsyncs. Only sync/syncfs wait for durability, and with a remote they also wait for the upload (TASK-165.11: about 130 ms on GCS).

The user asked (2026-11-06) for at least an option where fsync waits for local durability, without waiting on object storage, closing the extentd-crash and reboot windows at the cost of a local disk sync per fsync.

Shape to decide in the plan:
- A commit level between ordered and uploaded: durable locally (the record and data synced on the host), wired through the extent device (a COMMIT flag, or COMMIT_DURABLE split into local and uploaded).
- Where the option lives: a guest mount option (e.g. fxfs fsync=durable), a per-store or per-tenant setting in extentd, or both. It defaults to today's behaviour.
- Group commit: concurrent fsyncs share one local sync, so the cost is a disk round trip per batch, not per call.

After TASK-165.9, which rewrites Store::make_durable and the flush path.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 With the option on, an fsync that returned survives extentd being SIGKILLed: crash-dax-daemon-killed with the option loses no acknowledged file in 10 or more jailed rounds
- [ ] #2 With the option off, behaviour and performance are unchanged (the perf gate)
- [ ] #3 fsync latency and pgbench tps with the option on are measured on GCE against off and recorded
- [ ] #4 doc-12's tables say what each setting survives
- [ ] #5 After a crash (extentd killed, or the host rebooted with its disk kept), the store recovers every locally durable commit from the host's own store, its local WAL, before or without the bucket: a test cuts power or kills extentd and finds every acknowledged fsync after reopening on the same host
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-06, the user: in a crash the store must recover from the local store, so a locally durable fsync is worth something without object storage. This may be a large tax (a disk sync per fsync, or per group commit), so it's filed rather than scheduled; start it only when asked.
<!-- SECTION:NOTES:END -->
