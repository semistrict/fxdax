# Ordering framings in the framer so that recovery may trust any whole framed file

**Tried:** 2026-11-10, TASK-310, local only (never pushed): with framed segments made durable
together off the framer (`segments::make_durable`), the framer held back the framing of a segment
ending a commit that segments framed before it went on with, until those were durable
(`Framing::waits`), as each framing had been durable before the next one was written.
**Idea:** Recovery (`wal::recover`) took any whole framed file as its segment's record. A commit
spanning segments is certified by the footer of the segment ending it; written while an earlier
segment of the commit was not yet durable, a crash could keep the ender whole and tear the earlier
one, which recovery closed again from its log, lacking the commit, and the commit was read whole
with its runs there lost (`segments_made_durable_together_survive_crashes_making_them_durable`,
stopped after 13 operations, seed 1). Waiting for them before writing the ender closed that case.
**Result:** That test passed; the Shuttle crash rounds then failed (round 137: epoch 14 recovered
without extent 12). Commits that span nothing break the same way: segment 8, torn, its log's entry
never durable, went as holding nothing, and segment 9, framed after it and kept whole by the crash,
named commits 12-14 over the lost commit 11. Every later whole framed file is a record of commits
after an earlier lost segment's, not only a spanning commit's ender.
**Why it's dead:** It patched one instance of a class: recovery relied on framings being durable in
order, which the batching ends. Recovery now trusts a whole framed file only up to the first segment
that isn't whole, and closes the rest again from their logs, which hold every commit a flush made
durable; no framing waits for another's syncs.
**Reopen if:** Commits made durable only by their segments' framings, never flushed, must survive a
crash after a gap (nothing promises that today: `Store::uploadable` and every guest's fsync are
flush durability).
