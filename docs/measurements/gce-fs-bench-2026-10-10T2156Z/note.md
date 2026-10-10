TASK-25.7 follow-up experiment: libkrun sets KVM_HINTS_REALTIME in the guest's CPUID, so the guest polls before halting (cpuidle-haltpoll) rather than KVM polling for it; with TASK-25.9's caching (not committed).

Iterations: 3. Cases: bench-fs-fxdax-nodax bench-fs-passthrough. Workloads: metadata,build.

Finding: with the guest polling before it halts, every latency-bound workload is faster, on both
backends. fxdax without DAX (against 2026-10-10T2146Z): creates 2,193 -> 2,894 a second, cold
stats 4,949 -> 5,788, unlinks 3,552 -> 4,031, the build 7.68 -> 7.10 s. Passthrough (against
2026-10-10T2124Z): creates 1,476 -> 2,023, cold stats 10,079 -> 13,939, unlinks 4,147 -> 5,136,
the build 7.75 -> 7.45 s. KVM_HINTS_REALTIME also tells the guest its vCPUs are never preempted,
which changes its spinlocks; cpuidle_haltpoll.force=1 enables the polling alone.
