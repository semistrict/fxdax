TASK-25.4: profile of fxdax's VMM with DAX on the I/O workloads, whose sequential writes take most of the run, for where a DAX write's time goes.

Iterations: 1. Cases: bench-fs-fxdax-dax. Workloads: io. perf: record.

Finding: the most sampled is the host's TLB shootdowns (smp_call_function_many_cond, in the fault
thread and the executor's), KVM zapping its tables (tdp_mmu_zap_leafs) and flushing the vCPUs'
TLBs (vmx_flush_tlb_all): each change to the window's host mappings, made a page at a time for
changes of protection, invalidates KVM's mappings and flushes TLBs, interrupting the vCPUs.
