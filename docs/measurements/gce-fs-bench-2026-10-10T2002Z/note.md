TASK-25.3 step B: the in-process kernel maps runs of consecutive physical pages with one host mapping each, and the window's second stage likewise, with fxdax's DAX counters (not committed).

Iterations: 1. Cases: bench-fs-fxdax-dax. Workloads: io. perf: stat:kvm:kvm_exit,kvm:kvm_page_fault.

Finding: a host mapping now covers ~13 pages (DAXDIAG map-pages per map, near the 16 a kernel
mapping call takes at most). Cold sequential reads rose from 197 to 270 MiB/s, cold random reads
from ~8,300 to 31,752 a second (ext4: ~15,000), and random writes are back at ~5,400. KVM's own
faults, one per page the guest first touches, are what is left of cold reads.
