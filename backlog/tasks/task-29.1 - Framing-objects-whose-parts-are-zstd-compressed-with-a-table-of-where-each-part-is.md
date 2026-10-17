---
id: TASK-29.1
title: >-
  Framing: objects whose parts are zstd-compressed, with a table of where each
  part is
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 13:35'
updated_date: '2026-10-17 15:10'
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
- [x] #1 An object frames each 1 MiB part of its data as zstd-compressed or raw, whichever is smaller, with a table of each part encoding, stored offset and stored length, and a CRC32C of each stored part
- [x] #2 Reading a range of an object data decompresses only the parts the range touches, shown by a test
- [x] #3 Incompressible data costs at most the table, shown by a test of random bytes
- [x] #4 Truncated or corrupt objects and parts fail with an error, never a panic, shown by tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_frame: objects of stored parts (each 1 MiB part zstd level 1 or raw, whichever is smaller, at a 4 KiB-aligned offset), footer, a table of each part's offset, stored length, CRC32C of stored bytes and encoding, and a trailer with lengths, a magic and a CRC of the tail.
2. API: pack (parallel over parts), Frame/Table from a suffix, reading a range through a stored-bytes reader that fetches only touched parts, unpack checks CRC then decodes.
3. Tests: round trip, range read touches only its parts, random data costs at most the table, truncated/corrupt objects and parts error without panics.
4. Callers move with it (TASK-29.2/29.3), since the format is shared.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Format (crates/extent_frame): stored parts | footer | table | trailer. Each 1 MiB part is zstd level 1, or raw where zstd does not shrink it, at a 4 KiB-aligned offset (so a host reads it with O_DIRECT, and incompressible parts stay contiguous). Table entry: offset u64, stored length u32, CRC32C of the stored bytes u32, encoding u8 (17 bytes). Trailer (40 bytes): data length, stored length, footer length, footer CRC, table CRC (covering the lengths and footer CRC too), magic 'fxdaxob1'. API: Packer (front-to-back, parts compressed on scoped threads), framed, Frame, Parts (of a whole object or a suffix; read(range, read_stored) reads only touched parts), Stored::check/unpack. Checkpoint tables (table.rs) decode through data(). Tests: objects_hold_their_parts, ranges_read_only_their_parts, incompressible_data_costs_its_table, corrupt_objects_fail (every cut, every tail byte and a stride of stored bytes flipped).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extent_frame frames objects in 1 MiB parts, each zstd (level 1) or raw if no smaller, at 4 KiB-aligned offsets, with a table (offset, stored length, CRC32C of stored bytes, encoding) and a trailer carrying CRCs of footer and table. Parts::read reads only touched parts. Verified by objects_hold_their_parts, ranges_read_only_their_parts, incompressible_data_costs_its_table, corrupt_objects_fail; macOS 1964 and Linux 1974 tests pass.
<!-- SECTION:FINAL_SUMMARY:END -->
