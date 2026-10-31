# Wrong theory: deferring evacuation saves MAPs because evacuated pages are rewritten soon

**Tried:** 2026-10-25, TASK-109.1, branch exp/evacuate-delay (5492cf523,
fxfs.dax_evacuate_delay_ms), r112, pgbench-dax with 1 VM.
**Idea:** The WAL's tail page is written at every commit and moves to the next staging region at the
next write, so its evacuation MAP looked wasted. Waiting would skip those MAPs.
**Result:** delay 0: 1104/1027/1006/961/958 tps, evacuate MAPs 79-92k, requests 99-110k, regions
~300-440. 200 ms: 1154/1092/1036/1002/981, 73-89k, 56-69k, ~530-690. 2000 ms: 1213/1144/1058/1011/
1031, 75-94k, 58-72k, 1,700-2,500. Evacuate MAPs did not fall. Requests fell 40% and tps rose 5-10%
(one VM each, so a hint) only because deferring batches the MAPs.
**Why it's dead:** Evacuated pages are cold, not rewritten: the WAL's tail isn't what gets
evacuated. One MAP per written run is what the staging design costs, however late it is made.
**Reopen if:** A workload shows pages evacuated and then rewritten within the delay. The batching
effect is batched-evacuation.md.
