# Reach TASK-140's and TASK-44.21's states in guest/sim by tuning the fake's timing

**Tried:** 2026-11-01, TASK-149.14, branch test/sim-next.
**Idea:** an extent reads as sealed, then open again, as a device write reaches one no seal found
held. So a seal inside a transaction, a slower write reply, a longer COMMIT, tasks on a few pages
and small windows should have a reader map a region read-only under a writer.
**Result:** TASK-99 went red at seed 924 so, and 12,646 extents reopened in 1,500 seeds, but neither
mutant did in 4,000, and a probe in each branch counted 0 hits in 1,500 seeds.
**Why it's dead:** those states don't come from timing. Fxfs says a shared block is not open though
its extent is (`writable = is_open && !is_shared`, data_object_handle.rs), so a reflink gives one
extent read-only and writable pieces at once, with no race at all. The sim had no reflinks. With them,
the same probe counted both branches in 900 seeds.
**Reopen if:** never needed: the fake is what was missing.
