---
id: TASK-79.98
title: 'Import: OCI whiteouts delete additions from their own layer'
status: To Do
assignee: []
created_date: '2026-10-24 00:54'
labels:
  - review-2026-10-23
dependencies: []
references:
  - crates/fxdax/src/import.rs
  - crates/fxdax/tests/import.rs
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 368000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the 2026-10-23 adversarial review at bce42647. Severity P2; high static confidence, not dynamically reproduced. Importer::entry immediately removes ordinary whiteout targets, including replacements already added by the current layer (crates/fxdax/src/import.rs:223-224). Adding x before its whiteout loses the new x, although whiteouts only remove lower-layer content. Nested opaque case: lower layer has d/sub/old; upper layer adds d/sub/new then marks d opaque. Traversing existing sub does not record it in made (import.rs:314-335), so opaque recursively deletes sub including new (import.rs:385-394). Explicitly reintroducing a directory must also not preserve lower-layer descendants that opacity should hide. Imported contents therefore depend on archive ordering. Existing tests cover direct opaque children only (crates/fxdax/tests/import.rs:215-257). Related TASK-39.5; TASK-79.56 covers other import failures. Estimated fix effort medium; fix risk medium due to nested directory, hard-link and metadata semantics.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Ordinary whiteouts remove only lower-layer content and preserve same-layer additions and replacements regardless of archive ordering.
- [ ] #2 Opaque whiteouts remove lower-layer descendants while preserving same-layer nested additions under both pre-existing and explicitly reintroduced upper directories.
- [ ] #3 Regression tests compare equivalent archive orderings and assert surviving new contents and removal of hidden lower-layer entries; their correct-behavior expectations fail before the fix.
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 The fix batch passes cargo build or cargo check, cargo clippy, scripts/test.sh on macOS and scripts/test-gce.sh for the Linux workspace and applicable guest cases.
- [ ] #2 Changed upstream deviations are documented, scripts/upstream-changes.py --check passes, and generated files are updated through their generators.
<!-- DOD:END -->
