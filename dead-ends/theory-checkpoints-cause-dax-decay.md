# Wrong theory: Postgres's checkpoints cause pgbench-dax's decay over five runs

**Tried:** 2026-10-30, TASK-131.1, 2cf31e1f9 (experiment, not for main: checkpoint_timeout=1d,
max_wal_size=100GB), r272.
**Idea:** What decays is how much a commit changes the window, from run 3 on. The first checkpoint
rewrites heap pages into small extents, so reads would then map many small extents where they had
mapped the load's large ones.
**Result:** The churn did come from checkpoints: window_maps_reach 3294 3432 3077 2928 2940 against
4742 5028 20150 22386 18608. But tps barely gained: 1075.4, 977.1, 985.9, 990.8, 1000.9, run 5 1001
against 1017 with checkpoints and run 1 lower, on different hosts.
**Why it's dead:** Runs stay flat from run 2 at the level the checkpointed runs fall to, and run 1
alone stands above it. The churn is real but isn't what decays tps. r287 and r288 then found no
guest kernel symbol moving by more than 0.22 points between run 1 and run 5.
**Reopen if:** A host-side profile by thread, run 1 against run 5, points back at these remaps.
