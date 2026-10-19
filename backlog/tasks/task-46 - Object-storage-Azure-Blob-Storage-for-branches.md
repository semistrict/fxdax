---
id: TASK-46
title: 'Object storage: Azure Blob Storage for branches'
status: To Do
assignee: []
created_date: '2026-10-19 11:13'
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
- [ ] #1 A bucket URL for Azure Blob Storage (container and prefix) works wherever s3:// and gs:// URLs do: extentd, fxdax-gc, fxdaxctl
- [ ] #2 Creates and updates are conditional as doc-7 requires (create-if-absent, update-if-version), verified against a real or emulated Blob endpoint (Azurite, MIT)
- [ ] #3 extent_remote's bucket tests run against Blob Storage too
- [ ] #4 doc-7 names Azure among the supported stores, with how its credentials are found
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
