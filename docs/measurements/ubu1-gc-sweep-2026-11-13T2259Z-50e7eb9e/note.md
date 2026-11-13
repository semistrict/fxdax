TASK-395.5 AC #5: per-cell sweep over ~1M objects in 10k cells, peak memory and wall time by passes at once, on main 22602dbd1

Host: ubu1.

## Reading (TASK-395.5 AC #5)

- Peak memory 25-59 MiB at 1, 4, 16 and 64 passes at once over 1.0M objects in 10k cells (largest cell 955 objects, 627 KB): bounded by the passes in flight, not the bucket. The whole-bucket collector held every listed object in memory (~300-400 B each).
- Wall time: 4 at once is the fastest (320 s, then 160 s for the second sweep); 16 (492 s) and 64 (395 s) are slower than 4. Passes share one task, so their CPU work isn't parallel: TASK-395.15.
- Each pass over a changed cell makes 7 LISTs and 1 delimited LIST (70k LISTs a sweep), ~164 requests a cell in the first sweep (TASK-395.16). The second sweep deletes what the first compacted, once its grace period is due; cells were changed by the first sweep's compactions, so none skipped.
- Per-cell logs (sweep-*.log, 1.9 MB each) and remote.log are left out; the run's directory on ubu1 is in run.txt.
