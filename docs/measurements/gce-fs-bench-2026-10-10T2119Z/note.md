TASK-25.7 diagnostic: the VMM's context switches recorded with their stacks on the metadata workloads, fxdax against passthrough, for which threads switch out and why (device timing not committed).

Iterations: 1. Cases: bench-fs-fxdax-nodax bench-fs-passthrough. Workloads: metadata. perf: record:sched:sched_switch.

Finding (perf-*.txt): the device's thread switches out mostly in epoll_wait, waiting for the next
request, and right after writing the interrupt's eventfd, preempted by the vCPU it wakes: 39% of
fxdax's switches and 46% of passthrough's. fxdax's tokio worker also waits on futexes (11%), and
its port-receiver, io and io_uring threads switch too.
