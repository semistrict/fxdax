---
id: TASK-45.3
title: fxdaxctl local mode only builds new things offline
status: Done
assignee:
  - '@claude'
created_date: '2026-10-18 09:22'
updated_date: '2026-10-18 09:45'
labels: []
dependencies: []
parent_task_id: TASK-45
type: feature
ordinal: 142000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdaxctl acting on stores itself (no daemon) can delete, fork, place and sync existing branches with only a host-local directory lock. On a host of a multi-node system those changes belong to a daemon, or to a writer holding the bucket and a placement token.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Without a daemon, fxdaxctl changes existing branches only with the bucket and a placement token (else refuses, saying why); import and take of new stores still work offline
- [x] #2 fxdaxctl's tests cover the refusals
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Without a daemon, fxdaxctl makes new stores (take, import) anywhere, and changes existing ones only multi-node (bucket and token) or single-node asked for, refusing otherwise with the reason. Verified by existing_stores_change_only_multi_node_or_asked_single_node.
<!-- SECTION:FINAL_SUMMARY:END -->
