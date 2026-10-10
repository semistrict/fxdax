TASK-25.4: the second stage's changes keep new memory inaccessible, or read-only, until registered, and libkrun retries a vCPU's KVM_RUN failing with EFAULT for up to a second, logging each retry (not committed); with 2026-10-10T2035Z's write path.

Iterations: 3. Cases: bench-fs-fxdax-dax. Workloads: io.

Finding: no failure; vCPUs retried a KVM_RUN failing with EFAULT 12 times in the run (each
logged). Sequential writes rose from ~35 to 47 MiB/s (no DAX ~55).
