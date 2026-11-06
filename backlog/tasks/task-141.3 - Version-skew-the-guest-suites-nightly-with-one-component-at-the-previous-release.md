---
id: TASK-141.3
title: >-
  The VM process, the daemon and the guest refuse a peer of another wire format:
  no version skew
status: To Do
assignee: []
created_date: '2026-10-31 20:30'
updated_date: '2026-11-06 00:44'
labels: []
dependencies: []
parent_task_id: TASK-141
ordinal: 490000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
AGENTS.md (no migrations, the user, 2026-11-05) covers wire formats: a VM process, a daemon or a guest of another build is refused, not served. Nothing checks today. OPEN carries no wire version (crates/fxdax/src/extentd/wire.rs); a request a newer VM process adds is refused EINVAL by an older daemon and let be, as BUILD is (wire.rs:177-181; crates/fxdax/src/extentd/client.rs:136-140); and a reply that grows, as OPEN's did with the host's max_map_count in args[3] (0f3ae7a36), is read by an older VM process as whatever it was. The guest's driver and the device negotiate virtio feature bits (doc-4), which let a guest run on a device of another build.

Proposal: one wire version, said by the VM process in OPEN and checked by the daemon, and one device protocol version the guest's driver checks at probe; any other refused, both named. BUILD then says nothing the version doesn't, or is required.

Rewritten 2026-11-05 from "Version skew: the guest suites nightly with one component at the previous release" (daemon N with VMM N-1, VMM N with guest N-1, optional requests tolerating EINVAL), which AGENTS.md's rule forbids.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 OPEN carries the VM process's wire version, and a daemon refuses another with an errno and a log naming both, by test
- [ ] #2 The guest's driver refuses a device of another protocol version at probe, naming both
- [ ] #3 No request is optional: nothing lets a peer's EINVAL for a request it doesn't know be (client.rs:136-140)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: rewritten to fit AGENTS.md's no-migrations rule (75bd872ba): skew tested to work became skew refused.
<!-- SECTION:NOTES:END -->
