TASK-41: request-path hand-offs cut: the VMM completes extent requests on the thread receiving extentd's replies, and extentd serves a request on the thread receiving it; against gce-fs-bench-frame (us-east1-b, bucket in us-east4).

Iterations: 3. Cases: bench-fs-ext4-dax-pmem bench-fs-fxfs-guest-dax. Workloads: pgbench. 
