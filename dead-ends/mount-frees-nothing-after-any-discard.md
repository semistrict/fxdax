# A mount whose replay discarded anything freeing nothing it found dead until the discard is recorded

**Tried:** 2026-11-06, TASK-210, a local commit on replay-cut (rewritten before landing): replay passed
`Allocator::on_replay_complete` a bool, whether it discarded a transaction, in place of the extents
those transactions name (`Journal::extents_verified_by`, removed). `ExtentTable::settle_replay` held
every death until a recorded flush if it was true.
**Idea:** Two bugs (CI runs 41876989384 and 42196106048) were deaths at mount whose free the per-extent
bookkeeping let through before the journal recorded the discard. A rule that names no extents can't
miss one, and the mount's `flush_for_frees` frees them a few steps later anyway.
**Result:** A model hunt found a full device that mounted no more: "cut 2, during op 21: opening:
Failed to go on from the journal: reallocating the journal's tail ... No space" (seed
4758196049501471983, TIGHT; passes on main and with the per-extent rule; pinned as
`a_full_device_mounts_with_the_room_of_what_it_found_dead`).
**Why it's dead:** It isn't free. The mount renews the journal's tail (`renew_journal_tail`) before
`flush_for_frees`, as nothing may write the journal before then. On a full device that reallocation
needs the room of the extents a mount frees at once, which dead bytes count against.
**Reopen if:** Renewing the tail stops needing free room, say by freeing the old tail's extents
first, or the mount's frees come before the tail's renewal.
