TASK-25.7 diagnostic: the VMM's context switches, wakeups and migrations each 100 ms on the metadata workloads, fxdax against passthrough, with each device's time per request (not committed).

Iterations: 1. Cases: bench-fs-fxdax-nodax bench-fs-passthrough. Workloads: metadata. perf: stat:sched:sched_switch,sched:sched_wakeup,sched:sched_migrate_task,kvm:kvm_userspace_exit.

Finding: per cold stat, ~3.5 context switches in fxdax's VMM against 2 in passthrough's, with
~1 wakeup each.
