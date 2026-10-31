# Wrong theory: the MAP_AHEAD pre-fault run after the reply causes torture's zero reads

**Tried:** 2026-10-29 to 31, TASK-133 and TASK-131.3: vendor/firecracker 43e203825 (fxdax
faeec5186), timing-only variant dbbbfd82c. r255, r266, r271, r274, then the soak of 2026-10-31.
**Idea:** A KVM_PRE_FAULT_MEMORY made after the MAP's reply races the guest's next window changes.
Zero reads came only with it: 2 in 9 full rounds, against 8 of 8 clean and later 28 clean rounds
without it.
**Result:** append_while_mapped was i_size growing before the bytes (76ec99cf9). Torture's reads
stayed with the change (r266 round 6) until the soak read zeros on Firecracker 412e0b8d4, which has
no late pre-fault (round 2 of 10). The cause was a fault leaving with blocks moved and unmapped
(e4b6b24d7).
**Why it's dead:** The bug was independent of the pre-fault. A failure this rare, about 1 round in
19-30, isn't cleared or pinned by one set of rounds.
**Reopen if:** Zero reads return with it. The change itself (first read 0.646-0.652 -> 0.550-0.569
s) is not dead, only unpushed. Whether its race with later window changes is real is still
unmeasured.
