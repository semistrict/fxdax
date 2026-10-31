# Wrong theory: the WAL's writers queue behind each evacuation's hold on the file

**Tried:** 2026-10-26, TASK-91.16, counters in 8140da231, r132, pgbench-dax with 1 VM.
**Idea:** The evacuation holds the file's semaphore for writing across its MAP_MANY, and every
write's iomap takes that semaphore for writing, so the WAL's writes would wait on each evacuation.
**Result:** Writes waited ~5-6k times a run, 1.0-1.3 ms in all. Evacuations ~45-49k held the
semaphore 34-36 s a run. The time went elsewhere: the guest was near CPU-bound. Later r203-112w
counted the rest: 45,698 read waits, 17.83 s; 4,968 write waits, 1.6 ms; faults 0.
**Why it's dead:** The counters: reads wait, about one a transaction, ~390 us each; writes hardly do
(TASK-112.4).
**Reopen if:** The counters change, as they did under evacuation-semaphore-downgrade.md, where
writes waited 7.5 s.
