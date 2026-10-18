---
id: TASK-43.7
title: >-
  The daemon's own VM socket is the default tenant: one rule, a directory, for
  every tenant
status: Done
assignee:
  - '@claude'
created_date: '2026-10-18 07:08'
updated_date: '2026-10-18 07:08'
labels: []
dependencies: []
parent_task_id: TASK-43
type: feature
ordinal: 138000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
With tenants (TASK-43.4), the daemon had two rules for which stores a VM process may name: a tenant socket's directory, and on the daemon's own VM socket TASK-39.2's peer-credential check (the VM process's uid against the store's permissions) plus FXDAX_EXTENTD_ROOTS. The uid check separated VMs within a tenant, finer than the tenant boundary the user chose. The user decided (2026-10-18) on one rule: the default socket is the default tenant, FXDAX_EXTENTD_ROOTS its directories (none: anywhere), and the uid check goes, its tests with it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Connections to the daemon's own VM socket name stores in FXDAX_EXTENTD_ROOTS alone, by whatever path (symlinks, ..), or anywhere without roots (tests)
- [x] #2 No check depends on the connecting process's uid or gid; the daemon no longer chowns stores to it
- [x] #3 The module docs, the daemon's docs and the README describe the one rule
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Removed the peer-credential checks and chown (Peer, StoreAccess, authorize_in); connections of no tenant are checked against FXDAX_EXTENTD_ROOTS with the same in_dir rule tenants use. Deleted peers_name_what_its_permissions_let_them and roots_bound_the_stores_served (user approved), replaced by in_dir_tests::paths_name_stores_in_a_directory_however_they_name_them; daemons_with_roots_serve_no_stores_outside_them unchanged and passing. Verified on GCE (extentd, grants, control, tenants, isolation, fxdaxctl, handover tests).
<!-- SECTION:FINAL_SUMMARY:END -->
