TASK-41: the whole host profiled through pgbench, extentd built with frame pointers: where a commit's store phase (~125 us here, ~20 us without a VM for as much data) goes.

Iterations: 1. Cases: bench-fs-fxfs-guest-dax. Workloads: pgbench. perf: record-all. 
