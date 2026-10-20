Perf batch 2 with the guest kernel's haltpoll cpuidle (cpuidle_haltpoll.force=1, both cases): how much of the gap is wakeup latency (TASK-41).

Iterations: 3. Cases: bench-fs-ext4-dax-pmem bench-fs-fxfs-guest-dax. Workloads: pgbench,metadata,io. 

Findings: Fxfs 1,465 tps with haltpoll (1,407 without), ext4 more (1.26x -> 1.31x): wakeup latency is not what separates them. Not made a default: polling spends the host's CPU, which hundreds of VMs a host share.
