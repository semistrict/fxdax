---
id: TASK-153.12
title: 'To try: one trust boundary for layer records'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 23:09'
updated_date: '2026-11-03 11:26'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 559000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.9: journal and super-block records are validated, layer records trusted then checked piecemeal: key order, short bitmaps, entries_in, tombstones. Check each item once in take_item with the journal predicate, as upstream TODO fxbug.dev/42178223 asks. It costs a check on the hottest read path.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Prototyped, its cost on the read path measured
- [ ] #2 Decided from the numbers
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Bench first: layer_read_cost (persistent_layer.rs), a scan and seeks of a 200k-item object-store layer, in release; baseline.
2. Refactor: validate_mutation's object-store arm becomes ObjectValue::is_valid_for + ExtentValue::fits, shared.
3. Boundary: LayerKey::is_valid_item (default true; ObjectKey and AllocatorKey check their store's values), Key gains LayerKey; take_item refuses an invalid item as Inconsistent.
4. Key order: take_next refuses a key not strictly after the last or overlapping it (follows).
5. Tests: unit tests per invalid kind and order; fsck tests that wrote off-block items into layers made aligned; corrupt_tests: records no layer may hold, rewritten into a real image's layer as a writer would, each refused as read, none read differently; red with the boundary out.
6. Measure A/B (none / item / item+order), decide; record in OPEN_QUESTIONS.md.
7. scripts/test.sh, GCE all with fxfs-guest and fxfs-guest-checks, upstream-changes --check.
<!-- SECTION:PLAN:END -->
