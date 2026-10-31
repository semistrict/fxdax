# Fixing TASK-133 by always giving the page up

**Tried:** 2026-10-31, TASK-133's first fix. Dropped before landing.
**Idea:** A fault moved its page's block, then left with -EBUSY when the next seal shut the gate,
the range still mapping the block Fxfs had freed. Giving the page up at once means no stale mapping
outlives the move.
**Result:** It doubled torture's time, 55 to 120 s, on the debug kernel.
**Why it's dead:** The cost was too high when the page can be kept. The landed fix handles `moved`
at `out:` in `fxfs_dax_iomap_write`, mapping what moved and isn't mapped yet on every way out
(e4b6b24d7). r338: debug kernel with move checks, 15 of 15 through torture, signature 0 times. r339:
checks-dax, -delay, -small, crash-dax 6 of 6 each.
**Reopen if:** Nothing in this form.
