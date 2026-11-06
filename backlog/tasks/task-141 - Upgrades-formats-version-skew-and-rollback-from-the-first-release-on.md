---
id: TASK-141
title: >-
  Formats under the no-migrations rule: each refuses any version but its own, by
  name; daemons upgraded by draining their hosts
status: To Do
assignee: []
created_date: '2026-10-31 20:30'
updated_date: '2026-11-06 00:44'
labels: []
dependencies: []
priority: high
ordinal: 487000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
AGENTS.md, "No migrations, ever: formats change outright" (the user, 2026-11-05, 75bd872ba), replaces the two-release plan doc-11 held: no release reads, converts or upgrades another's data, and no component is made to work beside one of another wire format. What is left of upgrading: every persistent and wire format says its version and a reader refuses any other, cleanly and by name; and a daemon is upgraded by draining its host. doc-11 is rewritten to that (2026-11-05).

Where it stands, at main dde8d58d3:
- Store files: extent_store's FORMAT, "fxdax extent store 6" (crates/extent_store/src/layout.rs:16), any other refused by extent_store::Format, ENOTSUP (crates/fxdax/src/host_store.rs:555-558, older_stores_are_refused_by_their_format; TASK-165.6). Done.
- Branches in a bucket: extent_remote's layout::FORMAT 2 (crates/extent_remote/src/layout.rs:23), any other refused with Error::Format, older ones too (crates/extent_remote/src/record.rs:211, records_of_other_formats_are_refused; 064701057, TASK-165.3). Done.
- Fxfs: LATEST_VERSION 61 (crates/fxfs/src/serialized_types/types.rs:45), EARLIEST_SUPPORTED_VERSION 60 (types.rs:59): a version-60 filesystem still mounts and is upgraded, since 321fb94f2 (TASK-136.3). TASK-141.1.
- VM process and daemon: no handshake; BUILD is optional, an older daemon's EINVAL let be (crates/fxdax/src/extentd/wire.rs:177-181, crates/fxdax/src/extentd/client.rs:136-140). TASK-141.3.
- Daemon restart: every VM on the host exits 158 (ExtentdLost); SIGTERM reports NOT_SERVING and exits, closing no store (crates/fxdax/src/bin/fxdax-extentd.rs:112-145, crates/fxdax/src/extentd/mod.rs:125-131). TASK-141.4.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every persistent and wire format refuses any version but its own, naming it, each by a test
- [ ] #2 TASK-141.1, 141.3 and 141.4 done
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage (AGENTS.md, no migrations, 75bd872ba): rewritten from 'Upgrades: formats, version skew and rollback from the first release on'. TASK-141.1 rewritten to refusing Fxfs's version 60, 141.3 to refusing a peer of another wire format, 141.4 to the drain without N-1/N or rollback; doc-11 rewritten to the rule.
<!-- SECTION:NOTES:END -->
