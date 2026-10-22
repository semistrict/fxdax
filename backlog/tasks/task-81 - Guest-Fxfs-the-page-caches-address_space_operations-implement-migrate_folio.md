---
id: TASK-81
title: 'Guest Fxfs: the page cache''s address_space_operations implement migrate_folio'
status: Done
assignee: []
created_date: '2026-10-22 12:37'
updated_date: '2026-10-22 13:13'
labels: []
dependencies: []
priority: medium
ordinal: 255000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs_vfs_aops (guest/linux/fs/fxfs/super.c) has no .migrate_folio, so kcompactd's compaction warns, with a stack trace on the console: '0xffffffff8224c0c0 does not implement migrate_folio', WARNING at mm/migrate.c:1054 move_to_new_folio. Seen in fxfs-guest-checks (without DAX) on GCE, 2026-10-22. It also means Fxfs's page-cache folios can't be migrated, which hurts compaction and huge pages.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 fxfs_vfs_aops sets .migrate_folio (filemap_migrate_folio, or one of Fxfs's own if its folios carry private state)
- [x] #2 No 'does not implement migrate_folio' warning on any guest test's console
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
6dbc848: fxfs_vfs_aops sets migrate_folio = filemap_migrate_folio (Fxfs's folios carry no private state). Verified on GCE: fxfs-guest-checks's console has no 'does not implement migrate_folio' warning, where it had one before; all default guest tests otherwise pass; Linux workspace tests pass.
<!-- SECTION:FINAL_SUMMARY:END -->
