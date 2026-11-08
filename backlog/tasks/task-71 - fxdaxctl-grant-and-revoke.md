---
id: TASK-71
title: fxdaxctl grant and revoke
status: Done
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-11-08 05:59'
labels:
  - sandbox
dependencies: []
priority: low
ordinal: 180000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Only the gRPC API has Grant/Revoke; add fxdaxctl grant and revoke for tests and operators.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. control::Client::revoke beside grant. 2. fxdaxctl grant TENANT DIR [UID [GID]] (prints the socket) and revoke TENANT, through the daemon alone, refused without FXDAX_EXTENTD_GRPC as stats is; module docs and usage. 3. Tests in crates/fxdax/tests/fxdaxctl.rs: through a daemon (Linux) and without one.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08 (closure sweep): implemented in 1600277d0 on land-m1 (CI run 42447440838), not yet on main. fxdaxctl grant/revoke call the control API's Grant/Revoke (TASK-43.4); without a daemon both fail 'tenants' grants are the extent daemon's'. Tests: tenants_are_granted_and_revoked (Linux Docker: pass; a VM process opens its tenant's store through the printed socket, gets EACCES for another directory, the socket goes on revoke, a VM connected keeps its store, a second revoke fails 'revoking acme'), grants_need_the_daemon (macOS and Linux: pass; red before, the old fxdaxctl printing its usage). All 21 fxdaxctl tests pass in Linux Docker; clippy clean in the files touched; upstream-changes.py --check passes. Left for the landing: DoD after land-m1's CI, then Done.

2026-11-08: landed with land-all4 as d09e3fe13 (was 1600277d0); green main CI 42449438764 (land-all4, at 2b19077be, main 722ce0723), whose macOS and Linux workspace jobs contain it. upstream-changes.py --check passed; no upstream file changed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdaxctl grant TENANT DIR [UID [GID]] and revoke TENANT, through the daemon's control API alone (Grant, Revoke; control::Client::revoke added), refused without FXDAX_EXTENTD_GRPC. Verified by tenants_are_granted_and_revoked (Linux: a VM process opens its tenant's store through the printed socket, is refused another directory, the socket goes on revoke, a second revoke fails) and grants_need_the_daemon (red before); all 21 fxdaxctl tests in Linux Docker; green main CI 42449438764 (land-all4, at 2b19077be, main 722ce0723), whose macOS and Linux workspace jobs contain it.
<!-- SECTION:FINAL_SUMMARY:END -->
