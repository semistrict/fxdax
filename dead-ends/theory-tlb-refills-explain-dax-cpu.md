# Wrong theory: DAX's extra guest CPU is TLB refills, so huge pages close the gap

**Tried:** 2026-10-26, TASK-91.16, r137 (4ef3d04b0, FXDAX_GUEST_HUGE_PAGES=2M), one host.
**Idea:** r133 and r135 showed DAX's user time per transaction 29% above no DAX's, spread over every
symbol, with ~4.4 vCPU TLB flushes a transaction. Each empties the guest's TLB, whose refills walk
two-dimensional page tables under nested virtualization with guest memory in 4K pages.
**Result:** DAX 4K 1140/1070/1033/1003/979, 2M 1200/1104/1059/1032/1004 (+3-5%). No DAX 4K
1717-1741, 2M 1821-1882 (+7%), init 16.2 -> 10.5 s.
**Why it's dead:** Huge pages help no DAX more than DAX, so TLB refills aren't what separates them.
r138 then found every guest task slower under DAX, as interrupted vCPUs would make it.
**Reopen if:** Nothing in the theory. Huge pages themselves are worth having for sandbox VMs
(machine-config huge_pages '2M'); only the explanation is dead.
