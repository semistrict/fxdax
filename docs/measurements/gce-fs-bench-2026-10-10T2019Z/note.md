TASK-25.3 diagnostic: how long FUSE_SETUPMAPPING takes in fxdax, and mapping in it, averaged over 64 calls (not committed).

Iterations: 1. Cases: bench-fs-fxdax-dax. Workloads: io. perf: stat:kvm:kvm_exit,kvm:kvm_page_fault.

Finding (log.txt, SETUPDIAG lines): reading the cold file, a FUSE_SETUPMAPPING of 2 MiB took
~580 µs in fxdax, nearly all of it mapping the range's 512 pages (MAP_RANGE, host mappings and
KVM's pre-faults; without pre-faulting, mapping took ~0.33 µs a page, with it ~1.1). At 892 MiB/s
a 2 MiB range takes ~2.2 ms: the other ~1.6 ms is the guest's, the transport's, and the L0
hypervisor's, which fills its own tables for the guest a page at a time, unseen by KVM here.
