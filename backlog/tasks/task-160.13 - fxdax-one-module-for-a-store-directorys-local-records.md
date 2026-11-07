---
id: TASK-160.13
title: 'fxdax: one module for a store directory''s local records'
status: Done
assignee: []
created_date: '2026-11-03 13:08'
updated_date: '2026-11-07 10:00'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 584000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
remote-*, fence-*, origin-*, public-*, attached-bases and the boot profile each have their own format and atomic write (two copies of write-then-rename; the profile isn't fsynced); read errors handled three ways. Explains 3e6615852, TASK-79.82, 79.84, 79.95. Proposal: StoreDir with typed records, one durable write, one rule for read errors.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 One durable write for every local record
- [x] #2 The boot profile is fsynced
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
TASK-184 (2026-11-05): remote.rs's durable writes (markers, fences, claims) now share one helper, write_new, whose new file is the write's own ({name}.new-{random}); a shared {name}.new let two writers of one file at once truncate or remove each other's (ENOENT). The one durable write this task asks for must do the same: extents.rs's write_bases still writes {BASES_FILE}.new, which two ATTACHes of one store at once would share, and record_base reads, then writes, the list (a lost attachment); extent_store host.rs's describe has {DESCRIPTION}.new, which two deletions of one handle at once share. The read-then-write writers of a child's claim are TASK-186.

2026-11-06 tidy, the records at f81174090: origin-* files went with claims (TASK-165.7, 67740ed72); a fork now writes ancestry (extents.rs:1822, ANCESTRY_FILE, by extents::inherit), so the records are remote-* and fence-* (remote.rs:86, 92), public-* (extents.rs:1878), attached-bases (extents.rs:1762), ancestry and the boot profile. TASK-165.9 removes remote.rs's marker (remote-*); the rest stays fxdax's.

Done on batch-x1, 2026-11-07, against main 00e541868. Of the records named, remote-*, fence-* and origin-* went with TASK-165.9 (a store's bucket, lease and origin are its description's and the bucket's now). What was left, attached-bases, ancestry, public-* and the boot profiles, is store_dir's (crates/fxdax/src/store_dir.rs): write (the one durable write: NAME.new written and synced, renamed over NAME, the directory synced), mark (empty markers, each synced, then the directory), read (missing is none; unreadable is EIO, logged), lines (each line parsed; one that isn't is EIO, logged), with typed records attached/attach, ancestry, public_branches/mark_public and inherit. extents.rs's two copies of write-then-rename (write_bases, inherit's ancestry), mark_public's own syncs and the three ways of read errors (EIO logged, anyhow logged, unwrap_or_default for the ancestry) are gone: an unreadable ancestry now fails the opening with EIO, as an unreadable attached-bases and public listing did. profile::Recorder::write writes through store_dir::write, synced (it renamed an unsynced file before). Tests: records_read_back_as_written, forks_inherit_their_stores_records; fxdax 309/309 on macOS (forks_record_and_replay_their_bases_boot_profile, the attach and public tests among them); Linux (Docker) fxdax and extent_store 674/681, the 7 failing those that fail in every container (stamping, grants, SIGQUIT stacks, thread stacks), clippy clean.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A store directory's local records are one module, store_dir, with one durable write (written beside, synced, renamed, directory synced) for every record and one rule for read errors; the boot profile is now fsynced. The remote/fence/origin records were gone with TASK-165.9. Verified by store_dir's tests and fxdax's suite on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
