---
id: TASK-29.1
title: >-
  Framing: objects whose parts are zstd-compressed, with a table of where each
  part is
status: To Do
assignee:
  - '@claude'
created_date: '2026-10-17 13:35'
labels: []
dependencies: []
parent_task_id: TASK-29
priority: high
type: feature
ordinal: 95000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
crates/extent_frame frames every data object, local closed segments and bucket objects alike: data, footer, a CRC32C per 1 MiB part of the data, then a 16-byte trailer. Compressing each part on its own means a part no longer starts at part*PART in the object, so the framing needs, for each part, its stored offset and length. The CRCs should cover the stored bytes, so a fetch is checked before it is decompressed. A part that zstd does not shrink is stored raw.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An object frames each 1 MiB part of its data as zstd-compressed or raw, whichever is smaller, with a table of each part encoding, stored offset and stored length, and a CRC32C of each stored part
- [ ] #2 Reading a range of an object data decompresses only the parts the range touches, shown by a test
- [ ] #3 Incompressible data costs at most the table, shown by a test of random bytes
- [ ] #4 Truncated or corrupt objects and parts fail with an error, never a panic, shown by tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
