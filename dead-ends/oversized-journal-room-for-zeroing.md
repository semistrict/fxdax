# Asking for journal room for the whole 16 MiB of old-space zeroing

**Tried:** 2026-10-31, TASK-139 (xfstests generic/161). Replaced before landing.
**Idea:** Zeroing a 16 MiB journal's old space in one transaction, with the file an extent a block,
ran past the journal's end at a small commit as it extended the file. Keeping room for all of those
records would keep the transaction inside the file.
**Result:** It regressed test_read_written_super_block by draining the metadata reservation. The
sources give no number.
**Why it's dead:** Room for 16 MiB of zeroing's records is too much for a commit to ask for. Zeroing
a slice per commit needs none of it: MAX_ZEROED_PER_COMMIT, 512 extents, whose records fit well
within a chunk (e9482bc21). r346 then found zeroing on top of a large commit's room still left the
next commit short, fixed by keeping a chunk ahead for the journal's own transaction and the next
(b963ed64a). Test: test_commits_zeroing_old_space_stay_within_the_journal.
**Reopen if:** Nothing in this form.
