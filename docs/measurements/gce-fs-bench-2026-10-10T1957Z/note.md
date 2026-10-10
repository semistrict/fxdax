TASK-25.3 step A: SETUPMAPPING maps the pages the file has in memory (ZX_VMAR_OP_MAP_RANGE), with KVM's exits and page faults counted each second and fxdax's DAX fault thread's counters (not committed).

Iterations: 1. Cases: bench-fs-fxdax-dax. Workloads: io. perf: stat:kvm:kvm_exit,kvm:kvm_page_fault,kvm:kvm_try_async_get_page.

Finding: mmap random reads rose from ~13,000 to 667,000 a second, and mmap random writes from
~1,700 to 4,558. Cold sequential reads rose only from ~169 to 197 MiB/s: fxdax's faults are gone
from them, and what is left is KVM's own fault on each 4 KiB page of the window, ~34,000 a second
(log.txt). Random writes fell from ~5,400 to 3,567 a second, to be checked.
