---
id: TASK-44.1
title: 'cargo-mutants sweep of fxdax''s own crates, its surviving mutants triaged'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-18 04:06'
updated_date: '2026-10-18 04:07'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: medium
ordinal: 130000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Mutation testing of the existing code, the cheapest of the techniques: cargo-mutants (sourcefrog/cargo-mutants, MIT) rewrites the code a mutation at a time and runs the tests against each; a mutant no test kills is code the tests don't check. fxdax's own crates: extent_frame, extent_store, extent_remote, fxdax (not the vendored Fuchsia ones, whose tests are upstream's). store.rs has 2,300 lines and one inline unit test (more in crates/extent_store/tests); extent_remote's collector has none of its own.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 cargo-mutants has run over extent_frame, extent_store, extent_remote and fxdax with cargo nextest, and its report (caught, missed, timeouts, unviable, per crate) is kept in docs/measurements
- [ ] #2 Each missed mutant is killed by a new test, or recorded as equivalent with why, or filed as a follow-up task
- [ ] #3 How to run it is a script or make target, documented, with its run time
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. cargo install cargo-mutants (27.1.0, MIT).
2. cargo mutants -p extent_frame -p extent_store -p extent_remote -p fxdax --test-tool nextest --jobs 4 (3,589 mutants), output kept.
3. Triage missed mutants per file: a test that kills each, or equivalent with why, or a follow-up task.
4. A script/make target to rerun it (whole or per file), its time recorded; the report in docs/measurements.
<!-- SECTION:PLAN:END -->
