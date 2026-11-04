---
id: TASK-153
title: Structural changes the bugs of 2026-11-01 point to
status: Done
assignee: []
created_date: '2026-11-01 22:52'
updated_date: '2026-11-04 07:06'
labels: []
dependencies: []
priority: high
type: task
ordinal: 544000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Two audits, guest/sim, the model test and GCE found about 40 bugs on 2026-11-01, most in a few shapes. These are the changes that make those classes impossible. The first three were chosen to do now; the rest are to try, each with a prototype or a measurement before committing to it. See AGENTS.md: every bug questions the structure.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each subtask is done, or closed saying what the prototype or measurement showed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Every structural change the bugs of 2026-11-01 pointed to is done, each subtask closed with what its prototype or measurement showed.
<!-- SECTION:FINAL_SUMMARY:END -->
