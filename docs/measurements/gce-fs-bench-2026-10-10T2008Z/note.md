TASK-25.3 step C: KVM maps the window's pages for the guest as fxdax maps them (KVM_PRE_FAULT_MEMORY, with a vCPU of its own), with fxdax's DAX counters (not committed).

Iterations: 1. Cases: bench-fs-fxdax-dax. Workloads: io. perf: stat:kvm:kvm_exit,kvm:kvm_page_fault.

Finding: KVM's faults while reading the cold file fell from ~60,000 a second to ~1,000, and cold
sequential reads rose from 270 to 771 MiB/s, mmap random reads from 676,000 to 1.11 million a
second. Writes took more of KVM's faults (~20,000 a second against ~12,000): pages mapped
writable, for the store faulting on them, are no longer pre-faulted.
