# No evacuation at all, with idle ranges holding regions reclaimed under pressure

**Tried:** 2026-10-28, TASK-109.1 attempt 1 (recorded in TASK-112.5 and doc-9). Reverted, no branch.
**Idea:** Each seal's evacuation holds the file across a MAP and maps most written pages a second
time. Leaving pages in their staging regions removes those MAPs and holds, and LRU reclaim of the
ranges holding regions gives the space back when it runs low.
**Result:** Correct: delay-injected checks passed 5 of 5. Fast while region space lasted: bench VMs
1513 tps against 1364, above ext4 dax=always on virtio-pmem at 1496; 8 GiB run 1, 1306 against 1125.
It collapsed once the regions filled: 667 -> 210 tps at 2 GiB, regions full 225k times a run, every
write mapping its page alone.
**Why it's dead:** A staging region holds pages of many files, so letting go of ranges frees few
regions.
**Reopen if:** Regions can be freed without every file's range letting go of them, for example if
staging regions stop mixing files' pages.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): evacuation and staging regions
for writes are gone. Moot.
