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
**Reopen if:** The data loss no longer blocks it. Nobody has measured batching's tps since the fix;
it needs a new measurement on the current tree and must still beat the +3-6% it gave.
