# import-bench, one Fxfs thread against two, without fxdax's tests' features (TASK-92.47, TASK-320)

ubu1-import-bench-threads-2026-11-10 again, but built as fxdax-examples' example:
`cargo build --release -p fxdax-examples --example import-bench` at 760223b94 (the example as it
was, moved), which has neither fxdax's failpoints nor extent_store's `testing`, as
`-p fxdax --example` had (TASK-320). On ubu1 (16 cores, shared, host.txt; the tree there at
c8e4ff561, the same example), under `fxjob --bench` (cores 4-7 to itself);
`import-bench 100000 256 1024 THREADS`, three runs of each, alternating (output.txt).

| layer | 1 thread | 2 threads |
|---|---|---|
| large file, MiB/s | 1,650 / 1,729 / 1,731 (1,703) | 1,470 / 1,531 / 1,588 (1,530) |
| small files, entries/s | 14,544 / 15,278 / 15,293 (15,038) | 15,562 / 15,488 / 15,447 (15,499) |
| replacing them, entries/s | 9,390 / 9,823 / 9,845 (9,686) | 9,894 / 10,155 / 9,816 (9,955) |
| whiting them out, entries/s | 29,352 / 29,840 / 29,669 (29,620) | 31,171 / 30,651 / 31,314 (31,045) |

Two threads are about as fast: small files and replacing them 3% faster, whiting out 5% faster,
the large file 10% slower, all near the runs' spread. That is about what the earlier run found
(small files alike, replacing 4% slower, whiting out 5% faster). Against it, one thread is 8-15%
faster on every layer, but that run was not pinned to cores of its own, so the gain isn't
attributable to the failpoints alone. The default, one (`Options::executor_threads`), stands.
