# extentd-mapbench with fxdax's failpoints and extent_store's `testing`, and without (TASK-320)

What fxdax's examples paid for the test features their package's dev-dependencies turned on
(fxdax's self-dependency with `failpoints` and `testing`, and extent_store's `testing`): each
`fail_point!` of fxdax, extent_store and extent_remote looking its name up in the fail crate's
registry, and each lock shared beyond one store taking a process-wide lock as it is taken.

- *main:* `cargo build --release -p fxdax --example extentd-mapbench` at 604578a17 (main), which
  has them: its binary has 35 `fail::eval` and `panic_if_asked` symbols.
- *new:* `cargo build --release -p fxdax-examples --example extentd-mapbench` at 760223b94 (the
  example moved, unchanged), which has none of them.

ubu1 (16 cores, host.txt), `fxjob --bench` (cores 4-7 to itself), default arguments, 10 runs of
each, alternating (output.txt). Medians of the 10 runs' means, in µs:

| request | main | new | main against new |
|---|---|---|---|
| write | 70.4 | 69.3 | +1.6% |
| commit | 733.5 | 718.8 | +2.1% |
| map | 21.1 | 20.6 | +2.4% |
| unmap | 18.6 | 18.1 | +2.2% |
| write 4 KiB | 20.1 | 20.0 | +0.8% |
| read 4 KiB | 18.5 | 17.9 | +2.8% |
| commit ordered | 18.9 | 18.4 | +2.7% |
| free | 17.4 | 17.1 | +1.5% |
| write 128 KiB | 66.2 | 66.3 | -0.2% |
| read 128 KiB | 42.7 | 42.9 | -0.4% |
| commit ordered (128 KiB) | 22.3 | 23.0 | -3.0% |
| free (128 KiB) | 19.1 | 19.2 | -0.5% |
| fsync: copy 1 | 34.9 | 34.6 | +0.9% |
| fsync: commit 1 | 23.9 | 23.6 | +1.1% |
| faults: loads 32 | 137.1 | 136.6 | +0.4% |

The features cost a request about 1-3% of its time where it is small and the daemon's own (maps,
small reads and writes, commits), within each row's spread from run to run (output.txt), and
nothing seen where the copy or the disk is most of it. A release build now refuses them
(extent_remote and extent_store, TASK-320).
