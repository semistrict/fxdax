# Commits racing a super-block are what let it record progress its journal hadn't flushed

**Tried:** 2026-11-01, TASK-149.5, branch `test/model-hardening`.
**Idea:** The fix on the reliability base reads `journal_file_offsets()` under the writer's lock,
because "commits go on while the flush runs". So a short sequence with commits racing a compaction
should show the bug put back: `Op::FillWhileCompacting`, a small reclaim size, a small journal
chunk.
**Result:** 240 sequences and 15,024 cuts with the bug back found nothing, nor did the sequence
pinned for it (`a_super_block_records_only_what_its_journal_flushed`), once a forced compaction
stopped overlapping the flush task's. Printing the offsets either side of the flush showed why:
that sequence had the allocator's offset move 25 KiB in the window (`{5: 253285}` to
`{4: 278169, 5: 278866}`), which only another flush of the object can do. A commit alone adds an
entry at or past the checkpoint, which replay treats as it would have anyway.
**Why it's dead:** Commits aren't what moves the offsets: an object's flush ending in the window
is. That took two compactions at once, a bug of its own (`force_compact` beside the flush task's,
TASK-149.5), or a store flushing itself (`FlushReason::EncryptedMutations`), which only an encrypted
volume does, and no fxdax volume is. Reading the offsets early stays right; the model reaches it
now only with `flushing: true`, a store flush a plain volume never makes alone.
**Reopen if:** Anything else starts ending an object's flush outside a compaction: then the fill
with `flushing` is the test to look at.
