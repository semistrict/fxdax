TASK-25.4 step B, again: a write fault on a page not mapped resolves its page writable as before, and after waking the faulting thread maps the next page and the pages after it in memory read-only; a store fault on a page mapped read-only that follows on from the last dirties 16 pages. (2026-10-10T2029Z mapped the faulting page read-only first, and a vCPU's KVM_RUN failed with EFAULT.)

Iterations: 3. Cases: bench-fs-fxdax-dax. Workloads: io. perf: stat:kvm:kvm_exit,kvm:kvm_page_fault.

Failed as 2026-10-10T2029Z did, later in the first iteration's sequential writes: so not mapping
the faulting page read-only first.
