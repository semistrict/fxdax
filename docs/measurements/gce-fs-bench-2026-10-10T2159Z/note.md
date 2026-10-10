TASK-25.10 step A: libkrun's guest kernel command line on x86-64 Linux has cpuidle_haltpoll.force=1, so the guest polls before halting; with TASK-25.9's caching.

Iterations: 3. Cases: bench-fs-fxdax-nodax bench-fs-passthrough. Workloads: metadata,build.

Finding: the guests used the haltpoll cpuidle driver (IDLEDIAG lines, not committed). Against
2026-10-10T2146Z (fxdax) and 2026-10-10T2124Z (passthrough): fxdax's creates 2,193 -> 2,770 a
second, cold stats 4,949 -> 5,901, unlinks 3,552 -> 4,040, build 7.68 -> 7.42 s; passthrough's
creates 1,476 -> 2,039, cold stats 10,079 -> 13,867, unlinks 4,147 -> 5,608. The metadata gains
match KVM_HINTS_REALTIME's (2026-10-10T2156Z); the build gains less (7.10 s with the hint).
