# Mapping a writable region further ahead at once, so that writes rarely MAP

**Tried:** 2026-10-31, TASK-146.2, fxfs.dax_region_step, r367 against r364, async-commit pgbench-dax.
**Idea:** A write-path MAP holds the inode's lock for ~670 us, and the file's other writes queue behind it, 45% of a write's time. Mapping 16 times further per step should make those MAPs rarer.
**Result:** Step 1024: 2688/2578/2615/2559/2584 tps (mean 2605) against step 64's 2545/2536/2560/2623/2569 (2567), different hosts: +1.5%, within noise. `reach` MAPs barely fell, 26k -> 23k, and the lock wait stayed ~110 us.
**Why it's dead:** Reach isn't what writes mostly MAP for. ~130k of their MAPs were `range`, with `window_regions_full` hit ~120k times: region space runs out, so pieces fall back to the range's own space.
**Reopen if:** Regions stop running out and `reach` becomes the larger part of write-path MAPs.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): writes no longer MAP, so neither
`reach` nor `range` write-path MAPs exist, nor `fxfs.dax_region_step`. Moot.
