# Leaving freed window space mapped for the next MAP there to replace

**Tried:** 2026-10-24, TASK-91.16, r69 (kernel b6, 8 GiB window, one host, 3 rounds each arm), with
fxfs.dax_unmap_freed=1 for the old behaviour. Reverted.
**Idea:** r68 put ~36k UNMAPs a run at ~410 us, the pruned regions' window space. Skipping them
would save those round trips and shootdowns.
**Result:** UNMAPs 0, MAPs ~350 us (from ~305). tps equal: 917/817/819/791/813 against 917/837/833/
798/799, 982/868/843/803/817 against 956/852/816/798/818. `leave_out` stayed ~7% of the VMM's
samples with no guest UNMAP at all.
**Why it's dead:** The host's window map over a mapped range unmaps the old range itself and sends
the VMM STAGE_UNMAP then STAGE_MAP. A page costs one TLB shootdown however it goes, so moving the
unmap only moves it, and leaving the space mapped pins host memory for nothing. r138 reached the
same conclusion.
**Reopen if:** A stage operation replaces a mapping without the separate unmap.
