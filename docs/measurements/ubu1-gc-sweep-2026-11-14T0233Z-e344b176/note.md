TASK-395.15/.16: the sweep re-measured after per-directory file-bucket locks, passes on a multi-thread runtime and fewer LISTs a pass; main 79c8249e1, against ubu1-gc-sweep-2026-11-13T2259Z-50e7eb9e (c9b5ba549)

Host: ubu1.

## Reading (TASK-395.15, TASK-395.16)

Against ubu1-gc-sweep-2026-11-13T2259Z-50e7eb9e (c9b5ba549), same bucket shape:
- First sweeps: 1 at once 629 -> 602 s; 4: 320 -> 559 s; 16: 492 -> 190 s; 64: 395 -> 112 s. Second sweeps: 196/160/318/252 -> 281/178/92/53 s. Sweeps now get faster with passes at once up to 64; 4 at once is slower than before, one run each (TASK-395.17
TASK-395.15).
- Requests a first sweep: 1,641,382 -> 1,611,382 (3 LISTs fewer a cell: TASK-395.16).
- Peak memory 23-95 MiB, growing with passes at once (95 MiB at 64), still bounded by them, not the bucket.
- Per-cell logs and remote.log left out (15 MB); the run's directory on ubu1 is in run.txt.
