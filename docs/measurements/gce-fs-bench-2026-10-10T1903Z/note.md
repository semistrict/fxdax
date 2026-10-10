Experiment: fxdax's executor with 1 thread rather than 4 (DEFAULT_EXECUTOR_THREADS), to see what cross-thread wakeups cost each request.

Iterations: 1. Cases: bench-fs-fxdax-nodax. Workloads: io,metadata.

Finding: with 1 thread, cold stats rose from 2,518 to 3,219 a second and creates from 1,187 to
1,412 (against 2026-10-10T1843Z); unlinks stayed at ~2,700. Reverted afterwards: a candidate.
