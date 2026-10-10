TASK-25.3 diagnostic: KVM's exits and page faults counted each second, and fxdax's DAX fault thread's counters (not committed), for what a cold DAX read costs.

Iterations: 1. Cases: bench-fs-fxdax-dax. Workloads: io. perf: stat:kvm:kvm_exit,kvm:kvm_page_fault,kvm:kvm_try_async_get_page,kvm:kvm_async_pf_completed.

Finding (log.txt, a line of counts a second): reading the cold file (seconds 8 to 10), KVM took
~36,000 page faults a second, one per 4 KiB page of the window the guest touched, and fxdax's
fault thread ~2,700, each resolved in ~110 µs and mapping ~15 pages. So of each page's ~25 µs,
~7 µs is fxdax's userfaultfd faults and ~17 µs KVM's faults on pages already mapped in the host,
which nested virtualization makes slow (each is an exit to the L1 host). Every userfaultfd fault
is for a write: KVM faults pages in for writing (`async_pf_execute`). Sequential writes (seconds
1 to 7) take one fault per page, ~54 µs each, ~11,000 a second.
