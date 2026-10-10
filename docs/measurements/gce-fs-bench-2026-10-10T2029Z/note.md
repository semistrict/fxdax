TASK-25.4 step B: a fault on a page not mapped maps it read-only first, with the pages after it in memory, before dirtying it for a write; a store fault on a page mapped read-only that follows on from the last dirties 16 pages.

Iterations: 3. Cases: bench-fs-fxdax-dax. Workloads: io. perf: stat:kvm:kvm_exit,kvm:kvm_page_fault.

Failed: a vCPU's KVM_RUN returned EFAULT in the first iteration's sequential writes, and the
guest stopped; no measurements. Mapping a page read-only and then writable while a write fault on
it waits is suspected.
