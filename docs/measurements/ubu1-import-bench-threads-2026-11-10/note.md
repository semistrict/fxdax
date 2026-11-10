# import-bench, one Fxfs thread against two, without the extent store's audit (TASK-92.47, TASK-303)

`cargo build --release -p fxdax --example import-bench`, which a release build with the audit now
refuses (TASK-303), at 4a56a8be7 on ubu1 (16 cores, shared, load 5-6 during the runs, host.txt);
`import-bench 100000 256 1024 THREADS`, three runs of each, alternating (output.txt). The GCE runs
of 2026-11-08 (gce-import-bench-threads-{1,2}-2026-11-08) had the audit compiled in.

| layer | 1 thread | 2 threads |
|---|---|---|
| large file, MiB/s | 1,419 / 1,553 / 1,557 (1,510) | 1,595 / 1,540 / 1,225 (1,453) |
| small files, entries/s | 13,736 / 13,806 / 13,940 (13,827) | 13,853 / 13,777 / 13,776 (13,802) |
| replacing them, entries/s | 8,727 / 9,018 / 8,840 (8,862) | 8,589 / 8,476 / 8,462 (8,509) |
| whiting them out, entries/s | 25,913 / 25,926 / 25,602 (25,814) | 27,068 / 27,490 / 27,009 (27,189) |

Two threads import no faster: small files alike, replacing 4% slower, whiting out 5% faster, the
large file within the runs' spread. The default, one (`Options::executor_threads`), stands.
