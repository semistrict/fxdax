TASK-59/TASK-79.10: pgbench -i -s 50 on Fxfs (DAX, GCS bucket), seals remapping the window's live mappings at their own offsets; before, the primary-key build found aid 4831399 duplicated (gce-fs-bench-s50-fix-2026-10-22T0925Z).

Iterations: 1. Cases: bench-fs-fxfs-guest-dax. Workloads: pgbench. 
