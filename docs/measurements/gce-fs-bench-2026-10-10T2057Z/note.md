TASK-25.5 diagnostic: the guest's clocksource and its kernel's messages about clocks, with no workloads (not committed).

Iterations: 1. Cases: bench-fs-fxdax-nodax. Workloads: none.

Finding: the guest boots on kvm-clock and switches to the TSC (kernel log in logs/); its kernel
notes a ~166 ms jump in sched_clock at the switch, and its wall clock lags the host's by 170 ms.
