---
id: TASK-160.27
title: CI runs what the repo already tests
status: To Do
assignee: []
created_date: '2026-11-03 13:09'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 598000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
CI runs cargo nextest --workspace alone, not scripts/test.sh (the clippy check, 6 Shuttle suites, 25 Python test files under scripts/tests). guest/tests' host-feature tests (vm.rs, script_vm.rs) run nowhere. guest/sim's zloop seeds interleave only, not failures or smoke; its test lists are kept by hand in lib.sh, check.sh and mutants.sh. A mutant patch that stops applying drops silently. Explains 0a3843c8a (test_each_edit_finds_its_code_once never ran), bf82dcb37, TASK-120, 127. Proposal: CI calls scripts/test.sh; a guest/tests job per feature set; lib.sh lists tests from tests/*.c; zloop seeds failures too; a --check mode for every mutant runner.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 CI runs scripts/test.sh and guest/tests' host tests
- [ ] #2 zloop seeds failures; the test list derived
- [ ] #3 A broken mutant edit fails CI
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
