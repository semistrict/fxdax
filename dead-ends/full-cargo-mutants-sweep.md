# A whole cargo-mutants sweep of fxdax's own crates

**Tried:** 2026-10-18/19, TASK-44.1, scripts/mutants.sh and scripts/mutants-gce.sh, on GCE.
**Idea:** The cheapest way to find code no test checks: 3,617 mutants (fxdax 1,758, extent_store
984, extent_remote 604, extent_frame 271).
**Result:** Useful where it finished. extent_frame's and extent_remote's whole sweeps; extent_store:
1,260 mutants, 830 caught, 84 killed by new tests, 73 equivalent, 19 follow-ups unfiled. fxdax:
shard 0 of 6 done, shard 1 stopped at 312 mutants, 2 and 3 partial, 4 and 5 not run. Cost ~2 min a
mutant on fxdax and ~4 min on extent_remote, 2-3 jobs per n2-standard-8.
**Why it's dead:** Too slow, with production asks first: stopped by the user on 2026-10-19.
Cargo-mutants also can't run both feature sets at once, so Shuttle tests need a second pass.
**Reopen if:** Production asks are done. To resume: shards 2-5 with scripts/mutants-host.sh, then
fxdax's triage. TASK-44.19 does this by hand for the guest's C.
