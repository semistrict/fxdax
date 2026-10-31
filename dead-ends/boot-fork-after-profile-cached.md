# Booting a fork only once its whole boot profile is in the cache

**Tried:** 2026-10-17, TASK-38 (experiment, not committed).
**Idea:** A profiled fork read /usr in 1.01 s against a warm fork's 0.56 s. If the replay's timing
were the gap, finishing the replay before boot would close it.
**Result:** It still took 0.78 s, its 387 MAPs 179 ms against warm's 42 ms.
**Why it's dead:** What remained was not the replay's timing.
**Reopen if:** The MAPs of a just-replayed fork cost what a warm fork's do.
