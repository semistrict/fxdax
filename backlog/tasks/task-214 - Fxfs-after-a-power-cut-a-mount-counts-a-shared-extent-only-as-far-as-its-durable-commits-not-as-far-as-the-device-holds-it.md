---
id: TASK-214
title: >-
  Fxfs: after a power cut, a mount counts a shared extent only as far as its
  durable commits, not as far as the device holds it
status: Done
assignee: []
created_date: '2026-11-06 14:01'
updated_date: '2026-11-08 04:28'
labels:
  - bug
  - fxfs
  - wontfix
dependencies: []
priority: low
type: bug
ordinal: 652000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-213's check (2026-11-06). The model's new oracle, that Fxfs counts each extent at least as far as the device holds it, failed at the first cut of a_full_device_mounts_with_the_room_of_what_it_found_dead. The device held 81920 bytes of a staging extent Fxfs counted at 4096. doc-4's crash model keeps any subset of the last epoch's writes, so data written to the staging extent can survive while the journal blocks of the transactions that committed it are lost. The mount sizes the extent from its durable records alone. Only extents open in the crashed epoch can be affected (a staging extent is open only in the epoch it was minted in): at most STAGING_SIZE (2 MiB) each, uncounted until the extent dies. That undercount breaks the promise the metadata reservation makes on a full device. No record Fxfs writes can close this, as a crash may keep the data and lose any record of it. The mount has to learn how far the device holds those extents: a device request for extents' lengths (doc-4, the guest driver, Firecracker, extentd), asked of the extents minted since the last barrier, which the root could record. Neither READ (zeros past the end) nor INFO.used (inherited extents, frees kept for views) can tell. Until then the oracle runs only on filesystems no power cut touched (World::cut in model_tests.rs).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test where a cut keeps a shared extent's data and loses the journal of its last commits fails before the fix
- [ ] #2 After a cut, a mount counts every extent at least as far as the device holds it, and the model's oracle (counts_what_its_device_holds) runs on cut filesystems too
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The user's decision, 2026-11-06: leave it. After a power cut, up to 2 MiB a volume's staging extent stays uncounted, freed when the extent dies; not worth a device protocol change now. Recorded in OPEN_QUESTIONS.md under Fxfs.

2026-11-08 closure sweep: closed as won't fix on the user's decision of 2026-11-06 (above, and OPEN_QUESTIONS.md under Fxfs). The oracle counts_what_its_device_holds still runs only on filesystems no cut touched (World::cut in crates/fxfs-posix model_tests.rs).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Won't fix, the user's decision (2026-11-06): after a power cut, up to 2 MiB a volume of a staging extent may stay uncounted until the extent dies; closing it needs a device request for extents' lengths (doc-4, driver, Firecracker, extentd), not worth a protocol change now. No code changed; the model's oracle keeps running on uncut filesystems only.
<!-- SECTION:FINAL_SUMMARY:END -->
