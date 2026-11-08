TASK-142: what a pgbench run stores in objects of their own, by the extents' reach (Options::large_extent 4 MiB), fxfs-guest jailed with its store in GCS, as r353's

Iterations: 1. Cases: bench-fs-fxfs-guest. Workloads: pgbench. 

## Result (TASK-142, with gce-fs-bench-own-extents-fxfs-guest-dax-2026-11-08)

One pgbench run each (scale and times as scripts/bench.sh's pgbench group), jailed, each case's
store in a GCS prefix of its own, on a c3-standard-8 (fxdax-bench-r1022, us-east1-b), at main
0d6fde6a9's tree. What each run's daemon put in the bucket, listed by kind after the run:

| Case | Segments | Extents in objects of their own | All PUTs | tps |
|---|---|---|---|---|
| fxfs-guest (no DAX) | 37, 48.8 MB | none | 41 | 2,154 |
| fxfs-guest-dax | 35, 47.3 MB | 19, 2.2 MB (72-480 KB each) | 59 | 2,042 |

GCE r353 (2026-10-31), the non-DAX case jailed in GCS, had made 6,128 objects of their own
averaging 3 KiB. An extent goes in an object of its own by its reach (Options::large_extent,
4 MiB) still, but pgbench no longer makes small ones: deciding by the bytes an extent holds would
put these 19 in segments, 19 PUTs a run, $0.0001. It does not matter now; TASK-142's AC #2 holds
as shown not to matter.
