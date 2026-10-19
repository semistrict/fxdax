---
id: TASK-46
title: 'Object storage: Azure Blob Storage for branches'
status: Done
assignee: []
created_date: '2026-10-19 11:13'
updated_date: '2026-10-19 12:49'
labels: []
dependencies: []
priority: medium
ordinal: 145000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The extent daemon keeps stores' branches in a bucket named by URL (extent_remote::store), which supports s3://, gs:// and memory:// only. Deployments on Azure have no bucket to use. object_store, which extent_remote uses, has an Azure backend (feature azure); doc-7's protocol relies on conditional writes (If-None-Match, If-Match/generations), which Blob Storage offers through ETags.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A bucket URL for Azure Blob Storage (container and prefix) works wherever s3:// and gs:// URLs do: extentd, fxdax-gc, fxdaxctl
- [x] #2 Creates and updates are conditional as doc-7 requires (create-if-absent, update-if-version), verified against a real or emulated Blob endpoint (Azurite, MIT)
- [x] #3 extent_remote's bucket tests run against Blob Storage too
- [x] #4 doc-7 names Azure among the supported stores, with how its credentials are found
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
object_store's azure feature. extentd, fxdax-gc and fxdaxctl all reach buckets through Store::from_url, so az:// works in each. Azure serves no suffix ranges (object_store returns NotSupported before any request), which extent_remote mapped to Unavailable and retried forever: get_range now falls back to HEAD plus a bounded range. scripts/test-azurite.sh runs Azurite on port 10000 (object_store's emulator mode) and extent_remote's real-store test store::tests::azurite, which passes (0.49s): the conditional-write probe, branches' lifecycle, forks, checkpoints read by ranges, the collector. Verified: scripts/test.sh on macOS (2096 workspace tests plus the Shuttle suites); Linux on GCE b3 (2152 workspace tests and every suite; guest cases 18/18, fxfs-guest-suspend-dax passing 3/3 on rerun after one vsock harness flake); scripts/upstream-changes.py --check passes. No upstream code changed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Branches can live in Azure Blob Storage (az://container/prefix, AZURE_* credentials). Suffix reads fall back to HEAD plus a bounded range, as Azure has none. Verified against Azurite with scripts/test-azurite.sh; doc-7 updated. Commit d372bab.
<!-- SECTION:FINAL_SUMMARY:END -->
