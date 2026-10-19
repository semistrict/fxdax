# Working on fxdax

## Batch Rust changes; compile and test once per batch

Write a large batch of changes before compiling or testing anything: several features or
fixes, days of work, each still its own commit. Then compile once, fix what the compiler finds,
build the guest kernel once if the batch touches it, and run the tests once, on GCE
(`scripts/test-gce.sh`; Linux tests run there, not in Lima).

Do not alternate "write one change, compile, test, write the next". A compile of this workspace,
a guest kernel build and a GCE round each take minutes; paying them per change wastes most of
the time. Rust's compiler catches most of what a per-change build would, all at once at the end
of a batch.

- Read the code a change touches, and its tests, before writing it, so that the batch compiles
  with few fixes.
- Write the tests for each change with it, in the same batch.
- Then: `cargo build` (or `cargo check`) once, `cargo clippy`, the macOS suite
  (`scripts/test.sh`), and one GCE run of the workspace and the guest cases.
- Measure a change on its own only when isolating it is the point of the measurement (an A/B on
  one host).
