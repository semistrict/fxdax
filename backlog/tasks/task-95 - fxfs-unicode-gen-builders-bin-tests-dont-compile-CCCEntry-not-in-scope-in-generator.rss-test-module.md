---
id: TASK-95
title: >-
  fxfs-unicode-gen-builder's bin tests don't compile: CCCEntry not in scope in
  generator.rs's test module
status: To Do
assignee: []
created_date: '2026-10-24 12:42'
labels:
  - bug
dependencies: []
priority: low
ordinal: 380000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
cargo check --workspace --all-targets fails: crates/fxfs-unicode-gen-builder/src/generator.rs:154, test_types builds a CCCEntry the module doesn't import (the file's use ::types::{CCCEntry, ..} is in another scope). Unchanged since the Fuchsia import (7f8d6b69e). Found 2026-10-24 while type-checking a batch; the quick check excludes the crate until fixed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 cargo check --workspace --all-targets compiles fxfs-unicode-gen-builder's tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
