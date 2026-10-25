# SQLite DAX appends (TASK-92.28)

The safe append implementation improves batch100 throughput by 10–13% in a
controlled GCE A/B. Batch1 throughput is effectively flat, so the original
small-transaction performance goal remains open. All correctness checks passed.

The [baseline](gce-sqlite-2026-10-24/baseline/comparison.md) and
[profile](gce-fs-bench-2026-10-24T2105Z-07519990/note.md) identified a transaction on
every extending write, including sub-page appends into blocks already writable.
WAL/NORMAL batch1 spent 19.44% of guest workload samples in `fxfs_dax_map_write`.

Writable pages now accept those appends directly. The kernel marks their size dirty
for inode writeback, and fsync/fdatasync explicitly record the current size before
the filesystem seals its writes. The Rust operation updates size metadata without
the zero filling of an ordinary file grow.

Deferred size has to reach Fxfs before it copies a block for COW, changes the file's
size, or clones its extents. Otherwise a copy can discard the newly appended tail,
or an ordinary grow can zero it. Background writeback holds the invalidate lock
shared and the file's DAX semaphore while reading and recording its size: truncate
cannot interleave an older size update after shrinking the file. Extending iomaps
use the same semaphore. Writeback takes no inode lock, since callers may hold that
lock while waiting for writeback.

An explicit grow from a partial last page also seals the window's stores and drops
that page's old mapping before the ordinary Rust grow copies its block. Recording
the size alone is insufficient: the old window mapping can hide a zero-filled
replacement until eviction. This grow pays a filesystem barrier; page-aligned
grows and ordinary writable-page appends do not take this added path.

The regression checks cover:

- `append_writable`: subsequent sub-page appends issue no map-write transactions,
  and fdatasync plus eviction preserve their size and bytes.
- `append_size`: appends survive another file's seal followed by COW overwrite,
  ordinary grow, shrink, and reflink, including a read before the file's own fsync.
- `append_writeback`: concurrent syncfs, append, truncate, and fdatasync preserve
  the expected bytes and final size after eviction.

## Failed intermediate versions

The [first full run](gce-fs-bench-2026-10-24T2145Z-e71aaaa6/bench.log) exposed an
ordinary grow zeroing bytes already appended through the window. Replacing it
with `txn_update_size` corrected that operation.

A [later three-iteration run](gce-fs-bench-2026-10-24T2231Z-3a4bb213/bench.log)
passed the guest suites but failed SQLite recovery in iteration three: the database
was structurally valid, but its version sum was 226401 instead of 1 after killing
an uncommitted writer. Its throughput numbers are not evidence of a correct fix.

The five-iteration fsync-only follow-up at
`gce-fs-bench-2026-10-24T2253Z-ee830fbe` was canceled during its kernel build after
review found the remaining writeback/truncate and stale-size COW paths. It did not
validate recovery.

The expanded `append_size` regression subsequently found a grow/coherence failure:
after growing a 132-byte file to 200 bytes, reads through the window matched, but
sync and eviction returned 200 zeros. This exposed the need to seal the source
stores and invalidate the old mapping before ordinary grow replaces the partial
last block. The regression keeps its expected original bytes and zero-filled
extension.

## Final validation and measurements

The original `a6ee070d9` kernel and the patched kernel ran on the same disposable
`n2-standard-8` host, using the same warmed benchmark harness. Each ran five
iterations, first baseline then candidate. Because the first round showed an
ambiguous batch1 slowdown, a second round reversed the order without rebuilding:
candidate then baseline. The medians below include all ten observations per
kernel; both orders are retained, including unfavorable samples.

Raw reports:

- [Baseline, first round](gce-fs-bench-2026-10-25T0022Z-d10635f9/comparison.md)
- [Candidate, first round](gce-fs-bench-2026-10-25T0024Z-d9443ef9/comparison.md)
- [Candidate, reversed round](gce-fs-bench-2026-10-25T0028Z-d9443ef9/comparison.md)
- [Baseline, reversed round](gce-fs-bench-2026-10-25T0030Z-d10635f9/comparison.md)

Transaction throughput, baseline → candidate, in txn/s:

- DELETE/FULL batch1: 250.650 → 256.222 (+2.2%).
- DELETE/FULL batch100: 42.056 → 46.290 (+10.1%).
- WAL/FULL batch1: 584.741 → 590.365 (+1.0%).
- WAL/FULL batch100: 93.613 → 103.388 (+10.4%).
- WAL/NORMAL batch1: 12,012.608 → 12,185.105 (+1.4%).
- WAL/NORMAL batch100: 168.175 → 190.596 (+13.3%).

Batch100 throughput improved in both orders for all three modes. Batch1 sample
ranges overlap substantially; these measurements do not establish a useful
batch1 gain. The initial WAL/FULL batch1 median decline of 7.1% reversed to a
3.0% gain in the second round, so it is not reported as an established regression.
Scan medians changed by at most 1.1% across the four scan cases.

Checkpoint timing remains noisy. WAL/NORMAL batch1's pooled checkpoint median rose
from 30.151 to 33.127 ms (+9.9%); its sample ranges overlap. The full individual
samples, checkpoint measurements, and pooled medians are preserved in
[ab-summary.json](sqlite-dax-appends-2026-10-24-validation/ab-summary.json).

Both kernels passed every recovery, integrity, payload-fingerprint, and snapshot
check in all ten iterations. The patched kernel also passed `extent-guest`,
`fxfs-guest`, `fxfs-guest-dax`, `fxfs-guest-checks`, and `fxfs-guest-checks-dax`,
including `append_writable`, `append_size`, `append_writeback`, and
`append_while_mapped`. The workspace suites passed on macOS (2,509 tests) and
Linux (2,622 tests), plus their concurrency and Python suites. Workspace Rust
source did not change during the subsequent C-only resize correction.

`cargo check`, `cargo clippy` (with existing workspace warnings), formatting,
upstream-source validation, and structured source review completed. See the
[validation record](sqlite-dax-appends-2026-10-24-validation/validation.txt),
[final DAX checks](sqlite-dax-appends-2026-10-24-validation/dax-checks.log), and
[red grow regression](sqlite-dax-appends-2026-10-24-validation/grow-before-fix.log).
The patched kernel SHA-256 is
`d9443ef9c659d56839b77155e4e1dc1c11284fb2018c24f99023aab308523227`.

The original multi-host baseline is context only; performance conclusions here
use the controlled same-host runs. Process-kill recovery does not simulate power
loss. The fast-path mechanism is implemented and correct under the tested cases,
but meaningful WAL/NORMAL batch1 improvement remains unproven.

Stage replacement, evacuation request count, and mapped-ahead copying remain
separate work under TASK-92.58, TASK-91.16, and TASK-92.12.
