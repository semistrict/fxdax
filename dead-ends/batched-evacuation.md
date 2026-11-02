# Batching evacuations, one per 16 seals or 20 ms

**Tried:** 2026-10-28, TASK-112.4 (not landed); the failures are TASK-112.6.
**Idea:** Files would be held about ten times less often, each MAP_MANY carrying a batch's runs.
**Result:** pgbench-dax 2 GiB run 1, 1218 tps against 1181, +3-6% over runs; evacuations 54.6k ->
6.4k, read waits 17.8 -> 11.0 s. But checks-dax under fxfs.dax_delay_us=200 failed 2 of 8 rounds
(r202 run-20261028T040329Z-b6ec46f4 round 3, r201 run-20261028T041213Z-8a31b666 round 4:
append_writeback read zeros, random_ops mismatched) where main passed 5 of 5.
**Why it's dead:** It lost data as it stood, for a small gain. The loss was not batching's own: it
was TASK-121's host race (ExtentStore::write against a writable MAP opening the extent), which late
evacuations made likelier, fixed by cae97000a. With fxfs.dax_evacuate_every=16, checks-dax-delay
passes 8 of 8 rounds on main (r221) and fails round 1 with that fix taken out (r218).
**Re-measured:** 2026-10-31, the data loss long fixed, fxfs.dax_evacuate_every=16: r365 1150/1063/1002/979/1016 tps (mean 1042) against r362 1160/1063/1003/985/1031 (1048), different hosts: no gain. Evacuations 250k -> 101k, but maps barely fell, 738k -> 660k: batches hold the same pages. Holds 181 -> 120 s.
**Re-measured:** 2026-11-01, with write(2)'s time split, on main at 5ee91358e, fxfs.dax_evacuate_every=16 against no setting, four hosts of one zone, three rounds each: batched 1061 and 1054 tps, control 987 and 1021: about +5%, 3 to 7.5% by which hosts are compared. r365's "no gain" was two hosts, whose spread is as large. Evacuations 233k -> 100k, holds 178 -> 119 s, writes' wait for the file's semaphore behind evacuations 78 -> 57 s, MAPs not fewer, writes' own MAP time 26% up. A real gain, small, for the same reason: the cost is per page. Safe so far: checks-dax-delay's data checks passed one round, full DAX scripts three; but three checks that wait for an evacuation after one seal fail with it set (evacuation_paused, pmd_over_stale_index, evacuees_left), and one seal in 16 has no timer, so that a file gone quiet is never evacuated: shipping it needs the 20 ms timer back, and those checks told when it comes.
**Reopen if:** Evacuation's maps are made cheaper per page: the batching changes how many requests, not how many pages. Or a further 5% is worth the timer and the checks.
