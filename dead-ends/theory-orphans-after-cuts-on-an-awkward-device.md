# Extents unknown after a cut on an awkward device are orphans a mount left

**Tried:** 2026-11-01, TASK-149.3, branch `test/model-hardening`.
**Idea:** `holds_no_orphans` failed one sequence in about 300 once `FakeDevice` yielded: "4 extents
unknown". Mounts free what a crash orphaned, so one that left any looked like the seventh bug's
kin, and a failure that went away with a fix to the mount looked like that fix's evidence.
**Result:** Two rounds of hunting. A second sync and a wait for its frees, added to the check as a
probe, left none, and `has_dead_extents()` had said so all along. Those were extents dead and
waiting for a flush, or being freed by a task a sync doesn't wait for. A calm device finishes those
frees before the check looks; the pinned sequence I first kept as the regression test of the frees
fix "failed without the fix" only by this, and passed with the bug back once the check was right.
**Why it's dead:** `Fxfs::sync` starts frees rather than finishing them, and a device that yields
lets the test see that. The check loops over a sync and `Allocator::freed` until nothing is dead.
**Reopen if:** The check still reports extents with nothing dead: those are real orphans.
