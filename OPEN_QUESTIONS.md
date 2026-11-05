# Open questions

## gcloud needs a fresh login; fxdax-test-r632 is still up (2026-11-05)

gcloud's tokens expired mid-session ("Reauthentication failed. cannot prompt during non-interactive execution"), which only `gcloud auth login` fixes. Until then no GCE run, and `fxdax-test-r632` (us-east4-a, the perf gate's host) couldn't be deleted: it deletes itself 8 h after it was made, about 2026-11-05 23:15 UTC.
**Recommended:** `gcloud auth login`; then `scripts/test-gce.sh delete fxdax-test-r632` if it's still there.
**Until then:** unverified: dropping pgbench-ext4-dax-pmem's 8 GB image from the results (backed out, TASK-164.5's notes).

## The nightly needs GCP credentials in GitHub (2026-11-02)

`.github/workflows/nightly.yml` never ran: its YAML didn't parse (fixed in a1e079306). Now it parses, it needs the repository's `GCP_WORKLOAD_IDENTITY_PROVIDER` and `GCP_SERVICE_ACCOUNT` secrets and its `FXDAX_GCE_PROJECT` and `FXDAX_NIGHTLY_BUCKET` variables, none of which is set. Granting GitHub access to the GCP project is yours.
**Recommended:** a workload identity pool for `semistrict/fxdax` and a service account allowed to make and delete GCE instances and write the nightly bucket.
**Until then:** each nightly fails at its auth step; TASK-69's runs stay manual.

# Decisions

## Two mutants without killers since copy-on-write DAX: one equivalent, one killed in guest/sim (TASK-149.30, TASK-149.31, 2026-11-05)

- **`task-150.9-resize-moves-the-last-block` is equivalent copying on write**, and TASK-149.30 is closed with no killer, against its criteria, which asked for one. The flag it drops (`FXFS_SET_KEEP_TAIL`) changes only a cut to within a block, and `fxfs_dax_resize_begin` makes the last page private (zeroing past the new end through the window) before Fxfs cuts: nothing reads the moved block's old place through the window after. If a writeback and rebind come between, the block holds zeros past the end already, which Fxfs's `txn_zero_tail` leaves where it is. The mutant's header says so. *Instead:* a scenario that passes either way, which would kill nothing. *Undoing it:* reopen TASK-149.30 with a schedule this reasoning misses.
- **`task-79.49-store-fails-on-pins` is killed in guest/sim, not by a guest check** as TASK-149.31's criteria asked (GCE is out until the sign-in above). guest/sim now pins a page as `get_user_pages` does, a reference to the page (`sim_pin_user_page`), which `fs/dax.c`'s busy-page checks already see; `cow_store_to_pinned_slice` is the removed guest check `slice_held`'s steps, and the mutant says `Kernel: sim`. What the sim can't show is GUP's own races (its fast walk, a pin taken as a mapping goes). *Instead:* a check in fxfs-guest-checks-dax, unrunnable now and minutes a round. *Undoing it:* the mutant back to `Kernel: release` with a guest check named as killer; the pin can stay.

## extent_remote's simulation runs in a CI part of its own, optimized with debug checks (TASK-188, 2026-11-05)

CI's workspace part ran extent_remote's `sim::` tests unoptimized: four rule tests' seed searches took 20 to 27 minutes each and the simulation 7,434 of the part's 10,500 test-seconds (GitHub run of land-h). Decided: **`scripts/test.sh` part `sim`**, a matrix entry of its own in ci.yml (with the common build tools alone, TASK-172's), runs every `sim::` test (`package(extent_remote) & test(/^sim::/)`), and the workspace part every test but those. **Built with a Cargo profile `sim`**, release with `debug-assertions` and `overflow-checks` on, not plain `--release`: the simulation exercises invariants written as `debug_assert`s (`Wal::push`'s "a log's segments follow") and arithmetic a release build would wrap silently. **Only the simulation moves:** the logs' other slow tests (fxdax::handover 408 s, extentd's followers tests 204-227 s, fxfs-posix's model and corrupt soaks 72-154 s, extent_store::crash 112 s) are fixed-count workloads, not open-ended searches, and leave the workspace part near 15 minutes of tests. **The seed searches keep their bound of 20,000 seeds**, which already fails naming the range. *Instead:* plain `--release` (quicker to build, loses the checks); moving the other soaks too (a release build of fxdax and fxfs-posix in the job, for minutes). *Undoing it:* the part's lines in scripts/test.sh, the matrix entry, and `[profile.sim]`.

## guest/sim's model of Fxfs is checked against Fxfs by a Rust test that builds the model's C (TASK-149.29, 2026-11-05)

`crates/fxfs-sim-diff`: its build script compiles `guest/sim/fxfs/fxfs.c` as it is, with `cc`, over a shim of guest/sim's kernel of its own (`shim/`: libc memory, locks that never wait and fail the run if taken held, and kernel/device.c's extent store: lengths, seals, frees, no write to a sealed extent), and a proptest runs sequences of fxfs.h's calls on it and on fxfs-posix's `Fxfs` over a `FakeDevice`, comparing each file after each: size, bytes, every block whole through the extents, holes, sharing, and no extent open in Fxfs that the model says sealed. Operations run at the model's own `sim_point`s, as its locks allow, for the seal drift (seed 32826). It is a workspace crate, so CI's workspace part runs it on Linux and macOS, 64 sequences in about 15 seconds. Decided:
- **The model's C, compiled into a Rust test, not a C driver of fxfs-posix.** fxfs-posix is async Rust with no C ABI outside the guest's fxfs-linux, which needs the kernel's runtime; the model is 840 lines of C needing a dozen kernel calls. *Instead:* a C ABI for fxfs-posix on the host (guest/sim/README.md's "Linking the real Fxfs"), more work, and a C harness without proptest's shrinking. *Undoing it:* delete the crate.
- **A shim of its own, not guest/sim's**, which needs a kernel tree and Linux, so would run only in the guest-sim job. What it copies of kernel/device.c is the store's 60 lines; a drift there shows as a model that fails here alone. *Instead:* split kernel/device.c's store into a file both build. *Undoing it:* that split.
- **Mutants of the model are guest/mutants', a kind of their own** where nothing of guest/sim's kills them: `Kernel: model`, `Killer: fxfs-sim-diff TEST`, a test of the crate's (`scripts/guest-mutants-gce.py --check` checks it names one). guest/sim's `mutants.sh` runs `Kernel: sim` alone, as before; `crates/fxfs-sim-diff/mutants.sh` runs every mutant that changes the model, and fails if one of `Kernel: model` lives; its `--check`, in scripts/test.sh's scripts part, that each applies. *Instead:* `Kernel: sim` and `Killer: none` with a comment, which says nothing kills them, and nothing would run them. *Undoing it:* the kind's two lines in guest-mutants-gce.py and the README's.

## A host's WAL is placed by the guest's last commit through each segment; a fork's segment alone after the branch's last commit is taken as landed (TASK-165.5, 2026-11-05)

`extent_remote::Wal::rebase` puts a host's log after the branch's end by commits, as doc-14 and specs/bucket have it (`wal_follows`, `wal_after_fence`). Decided where they left it open:
- **A segment making no commit, a fork's alone, is placed by the commit before it**, and of the places the branch's last commit matches the last is taken as landed. If the fork's segment didn't land, the fork, never acknowledged, is lost, as a fork whose host lost its disk is; the other way it would be made twice. *Instead:* look the fork's child up in the fenced state's forks (a checkpoint drops forks whose children were deleted, so a landed fork reads as not and is made again); or give every fork a commit of its own (a change to what a fork is, for steps 7 to 9 to decide). *Undoing it:* the `rposition` in `Wal::rebase`.
- **The log's base after a rebase is the branch's last commit after the fence**, whichever commit decided how far it landed: what its segments stamped again follow, and what the guest goes on from.
- **The simulation drops a diverged log at once and its writer goes on**, as `fxdaxctl take` would and as the spec does; fxdax maps `Error::Diverged` to `ESTALE`, though nothing returns it there before TASK-165.9.
- **A session's open batch, the store's open segment, goes into the host's log when the session ends**, closed on its writer's chain, as reopening the store closes it (doc-14, step 3); with `Profile::restarts` (the `wals` profiles only) a host that crashes keeping its disk takes its branches back at once, as extentd does at boot, which puts the dead writer's creates between the new one's recovery and fence. Without it `wal_after_fence`'s first seed was 848, 70 s.
- **The simulation checks each live branch's history since its newest checkpoint**: every commit applied after its parent (the spec's `HistoriesOwn`). Before, a commit applied twice left the branch's contents as they were and nothing saw it; `wal_after_fence`'s seeds find it so.

## A deletion fences the branch at the end it recovered to (TASK-183, 2026-11-05)

Two deletions of a branch at once (a placement cut off and the latest one), a writer's fork segment in flight and never acknowledged: the first recovered the branch to the gap, found no fork and marked it deleted; a pass read it deleted and no record of the fork's child and let go of the branch's segments; the segment landed, and the second deletion, which had read the record live, recorded the child, whose origin the pass deleted. Decided, as specs/bucket modelled it: **`Rules::deletions_fence`**, `branch::delete`, having recovered the branch, creates an empty segment of the final epoch at the end it found (`recovery::fence`, which the writer's fence now shares), applying a segment landed there first and fencing after it, before it records the forks' children. Every deletion then finds the same forks, and every segment landing later follows none of the branch's. A point needs none. *Cost:* a create per deletion, and an object the next pass reclaims. *Instead:* the second deletion reading the record again before recording children, which still races the first's mark; or `end_lease` fencing creates, which conditional creates can't do. *Undoing it:* the rule's arm in `branch::delete`.

## The collector keeps every record, deleted roots' too (TASK-174, 2026-11-05)

A root that took the ID of a fork's child, and was deleted, lost its record to the collector; the fork's parent's writer, slow to record the fork, then made the child's record, naming the parent's segments the pass had reclaimed, and every pass after stopped. Decided: **`Rules::records_kept`**, no record goes, as deleted forks' children's already stayed (`Record::may_drop` is false). *Cost:* a record per branch ever made, small. *Instead:* drop a deleted root's record once no writer that could name its ID as a fork's child can still record, which needs a bound on how long a writer stalls, as doc-7 says of forks' records. TASK-177's restart, for a parent's record found gone, now guards only what would drop one; its tests run with the rule off. *Undoing it:* `Record::may_drop`'s rule.

## The collector recovers a branch by each origin it is named by (TASK-173, 2026-11-05)

A child the collector recovered by its parent's state, that fork refused because a restore of a point took the child's ID first, was not recovered again by the record of the restore's origin found on relisting, and what that origin needed went. Decided: **`Rules::recoveries_by_origins`**, the collector keys what it recovered by branch and origin, recovering a branch once by each origin its record or a parent's state names, and whether a segment or extent may yet land goes by every recovery of the branch. *Cost:* a refused fork's child is recovered twice in that pass, the state's way kept too. *Instead:* trust the record over the state, dropping the state's recovery when a record comes: what the state's recovery read may already be all the pass keeps of the parent, and the state's fork is never the child's then; keeping both is simpler and only keeps more. The simulation's restores now name pending forks' children (`restores_taking_forks_ids`), but no seed of 0..20,000 reaches the rule's window; the direct test and specs/bucket's `NoRecoveriesByOrigins` show it needed. *Undoing it:* `Live::recovered`'s by-origin arm.

## fxdax's additions to Fxfs's upstream files live in child modules, tests beside them (TASK-160.24, 2026-11-05)

data_object_handle.rs and volume.rs keep upstream's shape; fxdax's additions are child modules of them, impl blocks of their own (data_object_handle/{dax,write_many,reflink,in_commits,tail,write_once,snapshot}.rs, volume/{snapshot,base,lazy}.rs), each with its own tests. To reach upstream's test helpers, five of them became `pub(super)` (one line each), and the helpers fxdax's tests share went into `testing.rs` children. tail.rs keeps The Fuchsia Authors' header and cites upstream, as its zeroing is upstream's `grow`'s, moved; the rest are fxdax's own. Upstream diff: data_object_handle.rs +2276/-192 to +263/-178, volume.rs +1203/-146 to +47/-157.
**Instead:** fxdax's tests in modules nested in upstream's `mod tests` (`data_object_handle/tests/*.rs`), which needs no `pub(super)` but puts tests apart from their code; or one `fxdax.rs` child per file.
**Undoing it:** moving the impl blocks back; nothing outside the two files names the modules, as `ExtentAccess`, `FrozenPage` and `FAIL_COMMITS` are re-exported where they were.

## A volume's read-only is derived from its filesystem's, at one place (TASK-160.26, 2026-11-05)

FxFilesystem's options own read-only. fxfs-posix's `Fxfs::new` makes a volume read-only if its mount asks or its filesystem is; the guest's `Filesystem` reads the filesystem's options rather than keeping a copy; its `flush_volume` checks the volume's, which refuses a flush of a volume mounted -o ro on a writable filesystem (it checked the filesystem's only). The guest's `ExtentDevice` writes nothing only as `Filesystem::open` says (`writing_nothing`), no longer also from DEVICE_READ_ONLY at `find`; `read_only()` is `follows()`, what that flag says.
**Instead:** derive the filesystem's read-only from its device (`Device::is_read_only`) in `FxFilesystemBuilder::open`, which changes upstream code.
**Undoing it:** the guest's field back, and `Fxfs::new` taking its argument as it is.

## The perf gate's thresholds: doc-13's for tps, r632's for the rest (TASK-164.5, 2026-11-05)

Set from the first complete run, GCE r632: `density-memory` 0.9 and `density-memory-mib` 2560 (measured 0.862, 2,361), `bigmap-read` 4.0 (5.76), `bigmap-written-memory` 1.5 (1.29), `bigmap-guest-cache` 0.05 (0.0); `dax` 0.95 and `forked-dax` 0.9 stay doc-13's expectations, so the gate fails on `forked-dax` (0.573) until TASK-164.6 closes the gap. *Instead:* `forked-dax` from the measurement too, which would pass a 43% loss doc-13 says shouldn't be; or the density gates at 0.8 and 768 MiB, which DAX is far from (it saves 14% a VM). *Undoing it:* DEFAULTS in scripts/pgbench-gate.py.

## The daemon tells a VM process the host's vm.max_map_count; the perf gate runs jailed (TASK-172, TASK-164.5, 2026-11-05)

A jailed VMM can't read /proc, so its DAX window budgeted runs by Linux's default (30,717 runs where the GCE hosts allow 522,240). Decided: the extent daemon, which runs unjailed on the same kernel and already hands the VM process its memory in OPEN's reply, reads the sysctl and sends it in that reply (`args[3]`); the VM process reads no /proc for it. *Instead:* the jailer or Firecracker's caller passing it as an argument, which is VMM-specific and a second path to keep in step; or reading it before the chroot, which the jailer does, not Firecracker. *Undoing it:* `Opened.max_map_count` and its use in `create_dax_window_in_host_memory`.
And `scripts/perf-gate-gce.sh` now runs every Firecracker in its jailer's jail by default, as production does, so the gate measures what production runs (`FXDAX_FIRECRACKER_JAIL=` for unjailed). *Instead:* unjailed as before, faster to set up but not what ships. *Undoing it:* one line of the script.

## Every store names its bucket; images keep theirs in a file bucket in their directory (TASK-165.6, 2026-11-05)

A host store's description (`store`, local format 6) names its bucket's URL; `host_store::open` opens that bucket (one per URL per process, so a `memory:///` bucket is the process's). Stores and images made by older code, format 5, are refused by their format (`extent_store::Format`, `ENOTSUP`), opened or deleted alike, per AGENTS.md's no-migrations rule. Decided:
- **An image's own bucket is `<image>/bucket`**, inside its directory, which its clones share (their handles name the directory). Fine while the local lineage keeps an image's directory as long as its clones live; TASK-165.7, which removes that, has to move it (beside the image, or a bucket the caller names). *Instead:* `<image>.bucket` beside it now, which nothing removes with the last clone.
- **A store made on a host of fxdax's system names the host's bucket** (`FXDAX_EXTENTD_REMOTE`), `Storage::create` too (an import in extentd); elsewhere a file bucket of its own. A copy never placed takes its branch with `FXDAX_EXTENTD_TOKEN`'s token, or 1 where none is set: the daemon (token 1 in tests) and the CLI then take each other's branches, and neither takes one back from a later placement. *Instead:* images always in a bucket of their own, with an explicit URL for extentd's import.
- **Images' commits are durable locally, as before; uploads follow within the uploader's interval, and closing waits for them.** A commit fails once the uploader has (fenced, deleted). *Instead:* a flush waits for its upload, as `COMMIT_DURABLE` does: a segment per Fxfs flush, many small objects, which the collector doesn't merge.
- **The in-process collector runs for an image's own bucket only**, a pass on a thread of its own after a commit, at most one a minute, grace 60 s, as `fxdax-gc`'s defaults; a host's bucket (a daemon's, even a `file://` one) is left to its collector. Local cleaning stays until TASK-165.8.
- **extentd attaches a store to the bucket its description names**, an image's file bucket for an image it serves, and deletes there. scripts/test-e2e.sh's shared daemon keeps its stores in a file bucket on tmpfs (`/dev/shm`) rather than its memory, as the cases open its stores in their own processes (a memory bucket is its process's; a restore's claim of a point's fork found no point there). *Instead:* the snapshot cases on a daemon of their own; but every case opening a daemon's store needs this once uploaded data leaves the store (TASK-165.8).
- **The bucket is a line of extent_store's description, required of host stores** (`create_host` takes it) and absent from stores of memory (`Store::create`, extent_store's tests), which no bucket keeps.

## The TLA+ spec holds TASK-175 to 180's rules; it found two deletions at once recording a child after the collector let go of what it needs (TASK-183, 2026-11-05)

`specs/bucket/Bucket.tla` models `ways_from_seals`, `moves_taken_first`, `leases_by_takings` and `passes_fenced_stop`, each with its mutation, and TASK-177 and 178's fixes; the models as designed compact moves again. To reach those holes, steps the spec had merged are split as the code makes them: a taking may have read the record before the deletion (`staleTake`), a checkpoint's create lands after the writer takes its moves, passes overlap (`overlap`), a pass reads records as listings find them, and a deletion lists checkpoints before it recovers. That last split found TASK-183: two deletions of a branch, a never-acknowledged fork's segment landing between their recoveries, the second records the fork's child after the first marked the branch deleted and a pass, reading it deleted, let go of the child's origin. Decided:
- **The spec models TASK-183's fix as a rule the code lacks, `deletionsFence`** (a deletion's recovery creates an empty segment at the end it found, as a writer taking over does), as TASK-173 and 174 were modelled fixed; `NoDeletionsFence` was the code as it was. All three are in the code since (`Rules::recoveries_by_origins`, `records_kept`, `deletions_fence`; the decisions above). *Instead:* model the code as it is and leave BucketForks failing until TASK-183 is fixed, which stops every check of the spec; or have the second deletion read the record again before recording children, which still races the first's mark. *Undoing it:* the rule's arm in the deletion's `recover` step.
- **TASK-178's check is a spec rule (`deletionsDoneByOthers`) with no field of `Rules`**: the code holds it unconditionally (`branch::delete`), so no simulation test shows it needed; the spec's mutation does. *Instead:* a `Rules` field and a `test_needs`, a Rust change for TASK-178's next touch. *Undoing it:* drop the rule from `AllRules` and its mutation.
- **TASK-177's half of `newest_checkpoint` has a mutation of its own**, `NoNewestCheckpointInherited`, beside `NoNewestCheckpoint` (a segment missing): one rule, two ways it is needed.

## A PRIVATE refused with ENOMEM stays an EIO, not a wait (TASK-170, 2026-11-05)

GCE r582's pgbench saw `making 2 pages ... private: -12`, then EIO. doc-13 says the guest keeps under `private_max` and sends `PRIVATE` only within it, so ENOMEM there means the guest's and host's counts differ. That is a bug, here a KVM worker's write the host copied without being asked (TASK-170), and was fixed on the host. The guest still fails the write with EIO rather than throttling. *Instead:* throttle on ENOMEM as on its own budget. That would hide the next divergence as a stall that never gets room back. *Undoing it:* one branch in `fxfs_dax_dirty`'s failure path.

## A window's mappings carry no K: a source says what it is (TASK-160.11, 2026-11-05)

The task proposed that the window's `Mapping` carry `Dax`'s `K`, so that the store's second map could go. Decided: no `K` at all. Once writable DAX went, `K` (extents.rs's `Mapped`) held the source's identity, which the window's by-source index already has, and the extent a snapshot records, which the source, a cached extent, knows itself: `Source::origin` says it (an extent's ID, or zeros, replacing `is_zeros`), and `Dax::mappings` builds `WindowMapping`s from the window's own map. `Dax` is no longer generic, nor is `Window`.
**Instead:** `Window<K>`, generic through `DaxWindow`, the cache's window list (`Source::mapped`) and the userfaultfd stage, for a value the source already holds.
**Undoing it:** a field on `Mapping` and a type parameter on `Window`; `Source::origin` would stay for zeros.

## Late checkpoints: the collector's ways to extents start at their seals (TASK-175, 2026-11-05)

A checkpoint landing after a branch's writer recovered it (the writer it took the branch from, fenced and unaware, checkpointing late) names extents further along their moves than the writer does. The collector kept the moves only from where the state it read names them, compacted them in turn and deleted them: the writer lost the extents. Reachable on main (a direct test, red at 13db5c67c); a fork's inherited extents, and a writer's own checkpoint before it took its moves, had the same hole. Decided:
- **The way to an extent starts where it was sealed, every moves on it kept, unless the branch's newest checkpoint is by the writer holding its lease**, read after the checkpoints were listed; inherited extents' always (doc-7, Garbage collection, step 5). *Cost:* a lease GET per checkpointed branch per pass; moves compacted in turn stay while a branch's newest checkpoint isn't its lease holder's (until that writer's next one) or while forks without checkpoints of their own inherit through them.
- **A writer names what its checkpoint moves where it went before it writes it**, only forward along the way.
*Instead:* (1) a checkpoint counts only once a later footer names it: a format change, a fence rebasing on what it finds, recoveries reading an older checkpoint while the newest waits, forks no longer starting from the checkpoint at their fork; (2) a fence naming the checkpoint its writer recovered from: unsound (`dead-ends/fences-naming-their-checkpoints.md`); (3) keeping a moves a grace period after a pass last saw a state name what it is of: time, which doc-7 says correctness doesn't depend on.
*Undoing it:* `Rules::ways_from_seals` and `moves_taken_first`'s code (collector.rs's `behind`, writer.rs's `take_moves`).

The soak this was verified with, SIM_SOAK=100 of every holds test (TASK-165.3 had run 10), found four more holes, on main too: main's soak fails stale_placements seed 3953 (TASK-178's, the same failure) and bases_deleted_under_live_forks seed 1775 ("collector: lost: branches/2", TASK-177's kind). Each fixed as the protocol's other rules go, rather than by a structure of its own:
- **A recovery that found no checkpoint of a branch, finding what it inherited instead lost, starts again if one has come** (TASK-177), as for a segment gone. *Instead:* keep a deleted parent's record while any child might recover from it: no bound on that.
- **A deletion whose recovery of the branch fails, the record deleted since, is done** (TASK-178), as the collector takes it.
- **A writer's lease is its taking, epoch and holder** (TASK-179). *Instead:* never remove a deleted branch's lease (an object per branch ever deleted), or take a missing lease only for a record read after it.
- **A pass fenced by a later one stops without failing where it finds something lost** (TASK-180), as it stops before deleting. *Instead:* read the records again before recovering what a relisting found: narrower, and every relisting a round of GETs.

## Each guest check in a process of its own; the overrun suite runs the page cache's checks (TASK-160.30, 2026-11-05)

The guest agent (PID 1) ran every check of fxfs-guest-checks under catch_unwind, so a crash took the VM and the checks after it. Decided: the agent runs itself again per check (`FXDAX_CHECK` names it), in a process group of its own, to its budget; past it the guest's stacks are dumped and the group killed. Then, in one place (`guest::run::Guest::put_back`), it sets fxfs's writable module parameters back to what the guest booted with, thaws the filesystem and mounts it again if a check left it so, saying each, before waiting 10 s at most for the group; a group that outlives that resets the VM, as a hang did before. The host's verifies are now per check, in the table, and run for the checks FXDAX_CHECKS names too, where they ran only for a whole suite. fxfs-guest-checks-overrun now runs two checks made to fail (a SIGBUS, a write into a frozen filesystem) and then the whole page cache suite, about 60 s where it took 4 s, to show the rest pass after them.
**Instead:** fork per check without exec (unsafe in a process with threads); the overrun suite with a few checks after the failing ones (cheaper, and shows less).
**Undoing it:** `run_to` back to calling the body in-process; the overrun suite's `Suites::Failing` checks dropped.

## Guest test results as data: names, knobs, replay, bucket (TASK-160.31, 2026-11-05)

Each round's outcome is `CASE/round-K.json` (the runner's `test --outcome`), not `outcome.json`, so that a soak's passing rounds keep theirs beside `round-K.log` once their directories go; the run's `run.json` (commit, knobs, rounds, cases) and `summary.json` sit beside the cases (`scripts/guest_results.py`). Decided where the task left it open:
- **Knobs** are every `FXDAX_*` and `PJDFSTEST_*` test-e2e.sh is given, but those saying where it writes (`FXDAX_E2E_RESULTS`, `FXDAX_BENCH_RESULTS`, `FXDAX_COVERAGE`, `FXDAX_RESULTS_BUCKET`) and a kernel the host built (`FXDAX_GUEST_KERNEL` beside `FXDAX_GUEST_KERNEL_BUILD`); a case's own is `FXDAX_SEED`, from the `seed` file the checks and script cases already leave in their directory. **Replay** is `FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_TEST=CASE <knobs> scripts/test-gce.sh all`. *Instead:* each case declaring its knobs through `TestSetup` (touches every case, test_fxfs_guest_checks.rs among them, which TASK-160.30 is rewriting).
- **Failed checks** are read from the checks guest's `FAILED: a, b` line (and an overrun's `FAILED: a, still …`). TASK-160.30 must keep that line, or give the runner the names another way.
- **A mutant's fate:** killed if the summary has a case failed or timed out; a run failing otherwise (DAX refusals alone among them, as before) is BROKEN.
- **Uploads:** `FXDAX_RESULTS_BUCKET` (new; also gives the host the storage scope) gets `run.json`, `summary.json`, `cases/CASE.tar.gz` after each round and `logs.tar.gz` at the end, under the run's prefix; test-gce.sh fetches them when the host gave back no summary. An upload that fails says so in e2e.log and the run goes on. The nightly sets it to `$FXDAX_NIGHTLY_BUCKET/results`.
**Undoing it:** the formats are guest_results.py's and the runner's `outcome_json`; nothing else writes them.

## Guest test cases are selected by tags, the nightly runs 50 (TASK-160.29, 2026-11-05)

Every case is tagged with the suites it is in (`Tag`, guest/tests/test_cases/src/lib.rs); scripts pass `--tag SUITE` through `FXDAX_E2E_TEST` (or test-e2e.sh's arguments) and test-e2e.sh asks `runner list`. Each script's old list is now exactly its tag's selection; cases run in registration order, not each list's. Decided:
- **`manual`** tags the 16 cases no suite runs (xfstests, 4 hours each; runner-deadlock, failing on purpose; profiles, lock_stat, nofsync and the bench's followed and fork measurements), so that "untagged" stays an error.
- **The nightly** (`--tag nightly`) runs e2e's and kernel's cases plus crash-dax-daemon-killed, follow-remount(-dax) and the four script cases: 50, from 15. Left out: xfstests (hours), faults-dax (needs the debug kernel; soak-sanitized runs it), the measurements. The nightly still needs its GCP secrets (above).
- **`FXDAX_E2E_TEST=all` is gone:** it ran runner-deadlock and other cases that skip, which now fail a run asking for them by name.
**Undoing it:** a case's tags are a list beside its name; a suite is a variant of `Tag`.

## Once a batch of queued unlinks failed, every change fails, the data's too (TASK-160.23, 2026-11-05)

A failed batch leaves Fxfs maybe holding names the kernel took for gone; changes then failed with EIO until a remount, but only those through `transaction_with`. Clone (`clone_range`), writes (`write`, `write_many`), the page cache's writeback (`write_dirty`), `allocate`, truncates (shrinking or not) and the writeback of extended attributes made their transactions elsewhere and went on. Decided: each operation's row says whether it changes what the filesystem holds (`Op::rules`' `changes`), and every one that does fails: those seven too. The filesystem fails stop, as ext4 does once its journal aborts, by one rule rather than a list of exceptions. A writeback refused leaves the room its dirty data reserved as one that fails does: the page cache's pages are no longer dirty either way. Not refused: lookups, readdir, reads, `sync`, `fsync`, `statfs` (making what's there durable, or reading it), `set_xattr`/`remove_xattr` (kept in memory, their writeback refused), `evict` (frees nothing once a batch failed, as before), and the queue's own batches, which bring Fxfs back to the kernel's view.
**Instead:** leave data writes and cuts going: a file's data doesn't depend on its names, and a writeback refused loses what the page cache held; but what failed a batch is mostly the device, which fails them too.
**Undoing it:** `false` in the `changes` column of the operations to let through, and `every_change_fails_once_a_batch_failed`'s list.

## Local disk as the bucket's WAL and cache: how (TASK-165, doc-14, 2026-11-04)

The user chose the direction (review of 2026-11-04, decision 2); doc-14 decides the rest, where the task left it open:
- **WAL segments are stamped as they close**, by the writer's chain: each is the object byte for byte (doc-7, decision 1), and footers alone tell, after a takeover, which landed; the rest are restamped on the new chain if they follow the branch's end, by commits (changed by TASK-165.2, below). *Instead:* an unstamped WAL stamped at upload, which needs a record of every lease epoch the store took to tell its commits in the bucket from another host's: remote.rs's marker again. *Undoing it:* `wal::rebase` and the WAL's file format (TASK-165.5, 165.9).
- **A commit's local durability is a log beside the open segment** (a record of each commit's `Contents`, hashed, synced by the flush), folded into the footer when it closes. *Instead:* a segment closed per flush, 20 PUTs a second per VM; or keeping the index's deltas, a second encoding of the branch's state. *Undoing it:* TASK-165.9.
- **A WAL that doesn't follow the bucket refuses to open** (`Error::Diverged`, `ESTALE`) until `fxdaxctl take` drops it, as a stale store refuses now. *Instead:* drop it silently: the guest's restarted VM would see a history it never committed. *Undoing it:* one arm of the open path.
- **Capacity is branch state, set by commits** (footers and checkpoints): resizes are ordered with commits, forks inherit, points never change. *Instead:* a field of the record, updated by compare-and-swap, unordered with the commits it bounds. Format change (FORMAT 2).
- **One spending rule:** every footer free of the branch's own extents spends it (footers already free only extents a commit held). Remote recovery's "held before the segment" rule goes: it kept an ID sealed and freed within one segment writable elsewhere, which nothing needs, each writer minting from its own epoch's range. *Undoing it:* the rule in `apply_contents` (TASK-165.3).
- **Formats:** objects of any format but the reader's are refused, older ones too; today's local stores aren't opened or converted (AGENTS.md: nothing uses fxdax yet).
- **extent_store depends on extent_remote, and the uploader moves into it**, so that the host's side of the seam is tested against a memory bucket in one crate; `fxdax::remote` keeps the host's configuration, budgets, runtime and errno mapping. *Instead:* keep the uploader in fxdax, with its tests a crate away from the store they drive.
- **The part cache is kept across restarts**, its index rebuilt from its files: without a full local copy, an extentd restart would otherwise refetch every base.
- **The collector is the only cleaner;** the bound of three times the capacity on local files (TASK-79.89) goes with local dead space. The host's disk is bounded by the lag bound, the open segment's room and the cache's capacity.
- **Views (followers, frozen) keep working** on the new engine; a view reading an extent the collector reclaimed a grace period after its free is cut off. *Instead:* hold the collector off for views, which it can't see on another host.
- **The file bucket** is object_store's `LocalFileSystem` with conditional updates, and overwrites and deletes, under a bucket-wide `flock` (each holding opening the lock file anew, so that it excludes holdings in its own process too), e-tags being `LocalFileSystem`'s (inode, modification time in µs, size). *Instead:* a `Bucket` of our own beside `Store`, which `Store::from_url`, its stats and its throttle wouldn't cover. The e-tag could repeat only if a replaced file's inode were reused with the same size within its microsecond, the clock not going back.
- **The TLA+ spec comes before the protocol's code** (TASK-165.2, then 165.5), modelling the WAL's rebase; it is TASK-44.5's AC #1 too.
- **Steps 7 and 8 land in one batch**, so that main never has forks on one host reading their base back from the bucket.
- **A fork has no claim file:** its child's ID is checked with one GET; a fork whose answer a crash lost stands if its segment landed (the parent's next writer records it) and never was if the disk went with it; a fork fenced before its acknowledgement deletes its child by its owner (`branch::delete_fork`).
- **Images keep their branches in a file bucket named in their description**, their clones sharing it, their collector in-process (TASK-165.6).

## The WAL is put after the branch by commits, not by segment links; what the TLA+ spec found (TASK-165.2, 2026-11-05)

`specs/bucket/Bucket.tla` found that doc-14's `wal::rebase` by segments (the WAL segments whose footers recovery read are acknowledged; the first other one must follow the fence's `after`) declares a WAL diverged, and the store refuses to open (`ESTALE`, its commits dropped by `take`), when the branch holds every one of its commits but a later writer's checkpoint covers them (recovery reads no footer a checkpoint covers), or when only an empty fence came after them (a takeover that crashed after fencing). Decided: by commits, as doc-7's recovery on the same host already is: the WAL keeps the commit it was stamped over (its base); if the branch's last commit after the fence is one of the WAL's, the WAL's segments to it are acknowledged; if it is that one or the base, the rest are restamped after the fence; otherwise the WAL diverged. `wal_follows` and `wal_after_fence` are those two conditions; doc-14 says so. *Instead:* keep segments and have state and checkpoints carry, per lease epoch, the last segment applied of it, so that a WAL of epoch e knows what of it is in the branch: an entry per takeover kept for ever, and the empty-fence case still diverging. *Undoing it:* doc-14's step 4 and the spec's `Opened`.
The spec also found two holes in the collector, which it models fixed and which are tasks of their own, the code unchanged here: TASK-173 (a child recovered once, by its parent's state, though a record found on relisting names another origin) and TASK-174 (a deleted root's record dropped, then made again as a fork's child by its parent's slow writer; the spec keeps every record). It reproduced TASK-175, whose trace is now the mutation of its rule (`specs/bucket/mutations/NoWaysFromSeals.cfg`; above).

## The part cache's index is rebuilt from its files, which describe themselves (TASK-165.4, 2026-11-05)

The cache is kept across restarts (doc-14). Decided: no index file. Each object's file holds its tail, which says where its parts are; a part is kept only with its tail, and the tail goes with its object's last part (it is read after them, and spared while a part is being written). `Cache::open` locks the directory (`flock`, one cache at a time), walks it, and keeps each file whose tail, at its end, passes its CRCs; its parts kept are those whose range is all data by `SEEK_DATA`/`SEEK_HOLE` (an evicted part is a punched hole), those shorter than a block (which no punch frees) checked against their CRCs there, and all of them on a filesystem that doesn't find a punched file's holes (APFS reports what a resize left as data once a hole is punched: a probe at open). Files of no object, links and empty directories go. Least recently used order after a restart is the files' modification times. Then it evicts to the capacity and the floor. Every read checks: `Cache::tail` against the tail's CRCs, `Cache::part` against the CRC the tail gives, dropping what fails, so a torn or lost write reads as not kept and is refetched (`cache::tests::crashes_at_each_change_leave_nothing_read_back_wrong` cuts every change at each point). `Cache::adopt` renames a whole framed object in; it returns false and leaves the file where it was if it can't fit.
**Instead:** a durable index (a log of puts and evictions, synced), a second record of what the files already say, which a crash could leave disagreeing with them; or checking every part's CRC at start, reading the whole cache.
**Undoing it:** `Cache::open`'s walk (`Files::walk`, `found`) is the one place; an index file would replace it.

## A follower holds no entry in extentd's store table (TASK-160.8, 2026-11-04)

The task proposed every hold of a store as a value released on drop. Decided: a connection writing a store, and the daemon working on one, hold it so (`store_table::Hold`); a follower holds nothing in the table. The store connections have open is in the table as a weak reference, there while anything has it: its writer, its followers (through their `ExtentStore::following`), and the forks reading through to it as a base (`ExtentStore::open_bases`, in extents.rs), which a count of followers in the table wouldn't see. The table forgets it once nothing has it. As before, a store whose last follower drops it is closing as the table finds it gone, and a writer may open it then: its files' lock orders the two (`EBUSY` once it waited), not the table.
**Instead:** a follower's hold counted in the entry, kept beside its store in the connection's phase, and one for each fork reading through to a base, which reaches into extents.rs.
**Undoing it:** `Phase::Following` takes a hold, `Open::Followed` counts them, and its release forgets the store once the count is 0.

## A budget per guest check, its suite's limit their sum (TASK-168, 2026-11-05)

The checks suites ran under one fixed 180 s limit each, near it at times. Decided: each check has a budget in one table the host and guest share (`test_fxfs_guest_checks.rs`'s `checks!`): twice the most it took on the release kernel on GCE n2-standard-4 (r560's page cache, r563's DAX), at least 3 s. A check still running past its budget makes the guest dump its stacks, say `FAILED: <check>, still running after its budget of …` and reset the VM; the suite's limit is its checks' budgets plus 30 s, a backstop for a guest too stuck to. Budgets scale by the kernel's kind (`common::guest_kernel_slowness`: debug 1.5, measured at 1.37 at most; rust-checks and coverage 1.5, and KASAN 4, guessed), by 2 for object storage, and by `FXDAX_CHECKS_SCALE`. At this, the suites run at 16-21% of their limits (r564 release, r565 debug).
**Instead:** suites split into more VMs, each under the old 180 s (a split that drifts as checks are added, naming no slow check); a check failing over its budget only once it ends (a hang still waits for the suite's limit).
**Undoing it:** the budgets are numbers in the table; a check failing on its budget too often has its number raised, or `FXDAX_CHECKS_SCALE` set for a slower host.

## What holds a file's data is an operations table of its inode's; fsync stays super.c's (TASK-160.2, 2026-11-04)

data.c drives resize, punch and clone for both kinds of regular file, over `struct fxfs_data_ops`, which each inode points to as it points to its i_fop and a_ops: the page cache's set as the inode is allocated (super.c), the window's as fxfs_dax_set_inode makes it a DAX inode. So guest/sim links data.c with dax.c alone, no page-cache code. fsync isn't data.c's: its data half is one call, `fxfs_dax_write_back`, and the rest the VFS's metadata syncs, which guest/sim would have to fake more of than it would test. The model of Fxfs fails only growths, not cuts, as Fxfs's cut borrows the room it needs.
**Instead:** `IS_DAX(inode) ? &dax : &cache` in data.c, which would have guest/sim link a stub of the page cache's table; fsync in data.c, with the VFS's metadata syncs faked.
**Undoing it:** fold `fxfs_data_ops` back into `IS_DAX` branches in super.c, and guest/sim's mount.c's copies back.

## One replay, capacity by commits: how (TASK-165.3, 2026-11-05)

- **A resize is the next commit's, not a commit of its own:** `Store::set_capacity` keeps the local record as before and has the next commit's segment footer carry the capacity. A resize that committed would seal the extents a guest has open, whose next writes would then be refused. *Instead:* a commit with no seals at each resize, once commits no longer seal what's open (the open segment's log, TASK-165.9). *Undoing it:* `Uncommitted::resize`.
- **A store attaching to a branch whose state names no capacity, or another, restates its own with its next commit** (`Store::restate_capacity`), rather than in a segment of its own at attaching: attaching makes no more segments than before (fork tests name segments by number, TASK-157's). Until that commit, a host taking the branch elsewhere gives it the capacity it made its store with, as before. *Undoing it:* the diff's last plan commits the capacity (`Plan::commit`'s `capacity`), and the tests' segment numbers move by one.
- **A batch's footer has a store's shape:** an extent its commits sealed and later freed is a free alone, one a commit sealed and freed itself is neither (`batch::Plan::contents`). So the simulation's footers are the store's, and its new check of spent IDs sees the rule the store's footers need. *Undoing it:* `Plan::contents` keeps every seal.
- **A footer holds its segment's `Contents`, its extents of their own placed at its writer's keys by its place** (`Footer::own`): a footer naming its own extents elsewhere can't be made, and `Chain::admit` checks the place alone. *Undoing it:* the format, again.
- **The local record keeps the capacity until the lineage goes** (TASK-165.7); the store's local checkpoints and `rebuild` don't carry it.

## The host's copy-on-write DAX protocol once writable DAX is deleted (TASK-164.7, 2026-11-04)

Writable DAX is deleted from the host (vendor/firecracker's extent device, crates/fxdax, physmem, extent_store). Decided, where the brief left it open:
- **Daemon wire `WINDOW`:** no flag. `args[1]` is the private pages a window holds; a writer's window must copy on write (`args[1]` ≥ 1), a follower's must not (`args[1]` 0), `EINVAL` otherwise; the codec refuses any flag, and more private pages than the window has. The client's `create_dax_window_in_host_memory` takes `private_max` (0 for a follower); `create_copying_dax_window_in_host_memory` is gone. In-process, `ExtentStore::create_dax_window` (refusing every store) stays beside `create_copying_dax_window`, for followers and tests.
- **Retired numbers, not reused:** device feature bits 1, 9, 10, `MAP` flag bit 0, request 15 (`T_SEALED`); daemon requests 29 (`SEALED`), 107 (`SEAL`), 102 (`STAGE_PROTECT`), `MAP` flag bit 0, `MAPPING` flag bit 0, `WINDOW` flag bit 0. The config drops `seal_asked`: `private_max` follows `committed`. `STAGE_MAP`'s `args[3]` (its protection) is unused, ignored as words a kind doesn't use are. `STAGE_COUNTS`/`ACK` say four counts: write-protects went with seals.
- **`Protection` is gone from `SecondStage::map`:** every map is read-only; only `make_private` makes a page writable.
- **Stats and the control API:** `commit-write-ns`, `commit-written-bytes`, `commit-seal-*`, `stage-write-protects`, `stage-maps-open`, `open-alloc-*` are gone; `GetStatsResponse` reserves fields 16, 17, 19. scripts/bench-report.py reads `commit-seal-ns` unconditionally and needs the same change.
- **`FXDAX_CHECK_SEALED_READS` is gone:** it compared reads with what commits wrote of open extents' memory, which no longer exists.
**Undoing it:** none of this has a flag; restoring writable DAX is reverting the change.

## Copy-on-write DAX's private pages are the VM process's memory, laid out as the window is (TASK-164.1, 2026-11-04)

Each private page was a page of the tenant's arena, mapped in the VM process on its own: a mapping each, as the arena's frames aren't laid out by window offset, so the second stage's run budget (about 30k) trimmed and reinstalled them by the thousand; r544 held 110,857 and its fault thread trimmed instead of lifting a vCPU's restriction (`KVM_RUN` kept failing with `EFAULT`, exit 159). Decided: a window that copies on write has the VM process's private memory (`dax::uffd::PrivateMemory`), a memfd as large as the window, mapped shared at the window's offsets wherever nothing is installed, only private pages filled (`UFFDIO_COPY` through a second view, holes elsewhere so faults still go to the stage). `STAGE_PRIVATE` (108) has the VM process copy the source page (pread of the arena it holds) or zeros in; an unmap or map over it punches the hole. Adjacent private pages and the memory around them are one mapping; trims never touch them. The daemon keeps only their offsets and count; `private_max` is still pinned against the tenant's arena as the window is made.
**Cost:** the private pages' memory is the VM process's (its memcg), no longer in the arena's bytes: a tenant's limit no longer bounds it, `private_max` does.
**Instead:** (1) private frames allocated next to their window neighbours' (`alloc_after`): best effort, and scattered pages (pgbench's) still cost a mapping each; (2) a per-window physmem arena laid out by window offset: chunk-granular accounting charges 2 MiB per scattered page; (3) charging the memfd against the tenant's cap (physmem `charge`): possible follow-up.
**Undoing it:** `Window::copy` back to `physmem::alloc` + `map_run`, and `leave_out` to anonymous memory.

## The control socket is owner-only (TASK-159, 2026-11-04)

The control socket (`FXDAX_EXTENTD_GRPC`) followed the umask, world-connectable under the jailer's umask 000, and grants bound their tenants' sockets then chmodded them, a window at the umask's mode. Decided: the control socket is 0600, the daemon's user's, whatever the umask; a tenant's socket is 0660 (its jail's uid and gid) from the moment it appears at its path. Both go through `extentd::bind_given`, which binds in a private 0700 directory, sets the owner and mode there, and hard-links the socket into place (`EEXIST` if anything is there). No code or doc gave a group the control socket; every caller found runs as the daemon's user or root.
**Instead:** a configured group (a gid, mode 0660) for a VM manager of another user.
**Undoing it:** the mode in `fxdax-extentd.rs`'s call of `bind_owner_only`.

## DAX writes Fxfs's own blocks in place only if their record is raw (TASK-160.22, 2026-11-04)

**Superseded (2026-11-04):** writable DAX is deleted (doc-13, TASK-164); no window writes in place, `txn_map_write` and `window_access` are gone, and `in_place` serves `overwrite` alone.
A window wrote in place any blocks of an open extent the file alone held, whatever their record said. Blocks under a checksummed record (a growth's zeroed tail, ordinary writes) then held bytes their checksums didn't, which journal replay checks; blocks under an allocation's record read as zeros to Fxfs whatever the window stored. Decided: one rule, `in_place`, used by `extents_in`, `txn_map_write` and `overwrite`: in place only if the record is raw and nothing shares the blocks. Such blocks in an open extent are `ExtentAccess::Shared` to the window (read-only, the extent open), and a window's first write moves them once.
**Instead:** write in place and drop the record's checksums or allocation bitmap as the window writes: a transaction per first store.
**Undoing it:** `window_access`'s `InPlace::Recorded` arm back to `Open`.

## A layer's records cross one trust boundary, checked as they're read (TASK-153.12, 2026-11-03)

Layer records were trusted as read, then checked piecemeal where used. Decided: each item a layer file holds is checked as `take_item` reads it (`LayerKey::is_valid_item`), with the predicate the journal's mutations pass at replay (`ObjectValue::is_valid_for`, `ExtentValue::fits`; `AllocatorValue::is_valid_for` for the allocator's), and each key against the one before as an iterator advances (`follows`: after it, overlapping none). An item no layer may hold fails the read, a seek to it, fsck (`MalformedLayerFile`, also for keys out of order or overlapping, which fsck named on its own before) and the compaction meeting it. Cost, `layer_read_cost` on one GCE n2-standard-4 per comparison, three hosts agreeing to 1%: a scan 144.9 ns an item before, 157 with the item check (+8%), 165 with the order check too (+14%); seeks unchanged (1,050 to 1,200 ns either way). On an M-series Mac, under load, about +10%. Landed for both: without it, a record with checksums over its key, or keys overlapping, read back differently in 10 of 16 cases of `records_no_layer_may_hold_fail_what_reads_them`, with nothing saying so; the cost is on scans (compactions, fsck, a file's extents in turn), not on the seek every read starts with. Where the 12 ns of the item check go is unexplained: neither its two divisions (taken out, no change) nor, measured on arm64 only, the check alone (3 to 5 ns) account for it; moving the error paths out of line (`#[cold]`) wasn't measured, as gcloud's credentials expired. A seek still trusts order, as a binary search must: a key out of order may be missed by a point lookup, and only a scan finds it.
**Instead:** (1) keep checking where records are used: piecemeal, and wrong data where no use checks; (2) drop the order check, saving 6%: merges then trust order unchecked; (3) a check function each tree gives its layers, as `merge_fn`: threads one through every layer's opening, where a default method on `LayerKey`, downcasting to the store's value type (`dyn Any`, resolved at compile time), needs nothing threaded.
**Undoing it:** take the checks out of `take_item` and `take_next`; the predicates stay the journal's.

## Corruption is EUCLEAN and EBADMSG, not EIO; one errno table for both VFSs (TASK-160.21, 160.12, 2026-11-03)

Fxfs raises `Inconsistent` and `IntegrityError` on purpose now (TASK-153.11, 153.13), and both VFSs said EIO for them. Decided, in the one table both now call (`fxfs_posix::errno`): `Inconsistent` is `EUCLEAN` and `IntegrityError` `EBADMSG`, ext4's and XFS's `EFSCORRUPTED` and `EFSBADCRC`, logged. No caller depends on EIO for them: fs/fxfs's C compares Fxfs's errnos only with ENOENT, ENOSPC, ENODATA, EXDEV, EBUSY, ESTALE and the like, and no guest or host test expects EIO of a corrupt image. The rest of the table, where the two had drifted or said EIO: `AlreadyBound` EBUSY, `OutOfRange` and `InvalidVersion` EINVAL (ext4's mount of features it lacks), `NoKey` ENOKEY and `InconsistentEncryptionPolicy` EXDEV (fscrypt's). An error is classified by its outermost Errno, else its outermost FxfsError, context first; Fxfs's `matches` (root cause) agrees but for an FxfsError added as context. What only looked like corruption no longer says it: a manifest that doesn't parse or names no base is `InvalidArgs` (EINVAL; the guest's garbage-manifest check said EIO, and says EINVAL now), changes after a failed batch of unlinks fail with EIO, and a DAX write giving up on seals gives the seal's error (EIO). On the host, the uploader's failures keep `extent_remote::Error` (`Failure::Remote`) and map in one place (`Failure::errno`): `Displaced` is EROFS, as `Fenced` is, where it was EIO; a branch of a newer format (`Format`, TASK-141.2) ENOTSUP, until the host is upgraded; object storage's `Lost` and `Corrupt` stay EIO, doc-4's table having no other status for a store failing to read.
**Instead:** EIO for all corruption, as before (callers can't tell a bad disk from bad metadata); `EFSCORRUPTED` on the device's wire too (doc-4 would need a status).
**Undoing it:** the two rows in `crates/fxfs-posix/src/errno.rs` (`of_fxfs`) and their test rows.

## A DAX write(2)'s move commits after its bytes, as XFS's copy on write does (TASK-153.5, 2026-11-03)

**Superseded (2026-11-04):** writable DAX is deleted (doc-13, TASK-164); `fxfs_dax_begin_write` and `DaxWrite` are gone, and copy-on-write writeback commits after its bytes through `fxfs_write_many`.
A write(2) over the file's data moved its blocks and committed before its bytes were copied, so a write that failed after the move (a MAP failure, a seal, a killed writer, a copy cut short) left the new blocks' zeros where the data was (TASK-79.106, 119.13, 150.8, `map_failed_writing`). Decided: the move stays an uncommitted Fxfs transaction (`fxfs_dax_begin_write`, `DaxWrite`) from the iomap to `iomap_end`, where it commits out of the gate once the bytes are in, or is dropped, leaving the file as it was. The write holds its file's semaphore all along, and waits for faults past their iomaps first. Faults and zeroing still commit first. pgbench-dax measured equal: 826.3 against main's 834.3 tps over 25 runs a side on one host, the rounds alternating around 1 (docs/measurements/pgbench-dax-commit-after-bytes-2026-11-03.md). Landed for what it removes from write(2): bytes in hand before a move (-ENOTBLK), the -ESTALE retry after one, a second move after a seal, the second unmap of TASK-153.3; and a failed write can no longer zero data, which guest/sim's `failures` now checks.
**Instead:** keep committing before the bytes, with the retries that mend a failure after: the same tps, and more ways to lose data.
**Undoing it:** revert the TASK-153.5 commits (dax.c, fxfs.h, fxfs-linux, fxfs-posix's `dax_begin_write`, the sim model); the guest checks `write_killed` and `map_many_failed` go back to expecting the old behaviour.

## What a store may be asked to do is the store's, fixed as it opens (TASK-160.10, 2026-11-03)

Two holes were real: ResizeStore grew a point (the daemon opened it without its remote, which alone says it is one), and a failed listing of a store's public markers opened a public base private and writable. Decided: `ExtentStore::access()` (`StoreAccess`: writer, follower or point, and public) is fixed once the store is open and attached, and its methods check it: what changes what it holds (write, free, commit, map writably, clean, take over) needs a writer's store that isn't public (`EROFS` for a follower or point, `EPERM` for a public base); a resize needs a writer's, public or not, as no fork shares a capacity, but a point's forks take its; forking, making public and attaching a base, which change no extent, need any but a follower's, so a point may still be made public or have a base attached. A public base now refuses writes at the store too, not only a VM's open of it. Resizing a closed store attaches its remote, as every other daemon open does, so it takes the branch's lease (`ESTALE` if a later placement holds it). EROFS maps to FAILED_PRECONDITION. A marker listing that fails fails the open with `EIO`, logged. The default tenant is a real `Tenant`, nameless and unregistered for limits, with a policy of its own (roots, names branches, follows public bases).
**Instead:** keep each caller's checks and fix the two holes alone; or record point-ness locally, which would be a second copy of what the bucket's record says.
**Undoing it:** `StoreAccess::may_write` lets public bases through; `stores::resize` back to `open_without_profile`.

## CI runs scripts/test.sh, a part a job; open bugs' failures are listed, not failed on (TASK-160.27, .28, 2026-11-03)

CI ran `cargo nextest --workspace` alone. Decided: CI runs `scripts/test.sh`, which now has parts (`FXDAX_TEST_PARTS`: clippy, workspace, shuttle, scripts, and guest on Linux), as four jobs at once: the workspace, the Shuttle suites, the guest tests' own (as root, as on GCE: some mount loop devices), and the rest. The guest part runs guest/tests' own tests, both feature sets (vm.rs, script_vm.rs ran nowhere); scripts runs the mutant runners' `--check`. Every hand mutant's patch applies with no fuzz now, or fails `--check` (7 needed fuzz, one was malformed: refreshed). Open bugs' failures go in a file next to what meets them, `guest/sim/known-failures` (a regex of zloop's failure line) and `crates/fxfs-posix/known-panics` (a panic's place, which `corrupt_tests` counts apart); both are read on every run, CI's too, not only by the hunt, so that one bug fails nothing until its fix takes its line out. A nightly `hunt.yml`, CPU only: zloop over 20,000 new seeds (4 shards), the model and corruption tests at 50x (4 shards, seeds from the run's number); each night some 6 runner-hours, free on a public repo.
**Instead:** one known-failures file for both (their formats differ); known signatures only in the hunt (CI's 3,000 seeds then fail as often as an open bug shows: TASK-150.21's WARN, about one seed in 6,000, until patch 0012 fixed it); a hunt on GCE (needs the nightly's credentials, unset).
**Undoing it:** CI back to its `cargo nextest` step; drop `hunt.yml`; empty the two files. guest/sim's lists none now.

## A branch record's flags stay on the wire; it is decoded once into a kind (TASK-160.14, 2026-11-03)

A record's six flags (origin, forked, deleted, point, forking, claimed) were read as flags in four modules. Decided: they stay the bytes in the bucket, unchanged (FORMAT 1), and are decoded once into `record::Record { format, owner, kind, deleted }`, with `Kind` one of Root, Own, Fork, Point, Forking, Claim. Any other combination is refused as corrupt (52 of the 64). What a record allows is a query on it (`writable`, `is_child_of`, `may_drop`, `branch::lost_is_ok`), and every create or update of a record or a lease goes through one compare-and-swap loop, `branch::transition`. `branch::ForkedAt` is gone: `delete_fork` takes the `Kind` its child's record is or would be, and fxdax's claim holds that kind once its fork's segment goes. The suspected bug (a direct fork and a fork of a point at one segment, given one child ID, taking each other's records) was real until TASK-153.9's owners; a test now shows both that it no longer happens and that it does with `records_owned` off.
**Instead:** put the enum itself on the wire (serde's tag), so that no illegal record can be written at all; but decoding already refuses them, and the flags are what doc-7 describes.
**Undoing it:** fold `record.rs` back into `layout.rs` with public flags; nothing on the wire changes either way.

## The daemon asks other guests to seal when a writable MAP is refused (TASK-153.15, 2026-11-03)

**Superseded (2026-11-04):** writable DAX is deleted (doc-13, TASK-164); with no open pages there is nothing to seal for, and `F_SEAL`, `seal_asked`, `SEALED`, the daemon's `SEAL` and `extentd/seals.rs` are gone.
Only a guest can seal its open extents, whose pages count against a budget its daemon's VMs share. One idle VM's open pages could take the budget, and another VM got `ENOMEM` from writes its own seals couldn't make room for. Decided: when a writable `MAP` is refused, the daemon asks the guests holding the most open pages to seal. It asks those of its tenant first, enough to cover the `MAP`. It waits for their answers, holding no thread, 10 s at most, then tries once more. This uses a device notice (doc-4's `F_SEAL`, `seal_asked`, `SEALED`), shaped like the resize notice. The guest seals with an ordered commit. Asks are made only under pressure, not on a timer for idle VMs: the most-held ranking already picks the idle VM, and a timer would seal VMs nobody needs room from. A guest that has been asked never waits for another guest's seal, which rules out a cycle of waits. A guest that doesn't answer within 10 s isn't waited for again until it does.
**Instead:** (1) a per-tenant share of open pages: a VM still can't go past its share while others sit idle, and the shares need sizing. (2) Evict open pages to the store and fault them back: the daemon would have to take writable pages away from a running guest. (3) Also ask idle VMs on a timer.
**Undoing it:** in `Connection::map_writably`, stop calling `seals::made_room`; take `F_SEAL` out of the device's features. The guest then never sees an ask.

## The extent store's audit runs in every test build that links it (TASK-160.16, 2026-11-03)

`State::audit` recounts the ~15 byte counters from everything the branch holds each time its state's lock is let go, in builds with the `audit` feature. Decided: `shuttle` implies it, and `extent_store` turns it on for its own tests by a dev-dependency on itself. Cargo unifies features in one build, so a workspace test run (`cargo nextest run --workspace`, `scripts/test.sh`, GCE) audits the stores of fxdax's and the other crates' tests too: more checking, at the cost of a walk of every extent per critical section. Only branches holding at most 4,096 extents are audited (`AUDITED_EXTENTS`). Unbounded, fxdax's `counting_followers_walks_no_extents`, which writes a 32,768-extent base one extent at a time, ran past an hour on GCE. Release builds and the daemon never audit.
**Instead:** `cfg(debug_assertions)`, which also audits debug daemons and guest tests, whose stores are large; or an opt-in each test calls, which misses the ones that don't.
**Undoing it:** drop the self dev-dependency (the Shuttle and lib tests still audit), or `audit = []` and the `cfg`s.

## The journal's end is in the root, set by each sync's barrier (TASK-153.13)

A flip mid-journal read as its end, losing synced transactions unsaid. Decided: the root records how far the journal was written when a barrier set it (`Root::journal_end`), and a replay ending before there is refused as corrupt; past it, a torn tail is still a torn tail. A sync's barrier sets it: its journal write's `WRITE_COMMIT` carries the root (`WRITE_ROOT`, new on the wire), or its flush becomes a commit of the root. Cost: no request more; the root's ~50 bytes in the sync's request and the host's commit record; barriers setting roots take turns (they did at the device already). A synced write whose data the journal's checksums find damaged now refuses the mount, where it rolled the file back.
**Instead:** a durable marker only on `sync`/`syncfs` (fsyncs unprotected), or an extra `COMMIT` per fsync (a round trip each), or no marker, leaving it to the host's CRCs, which bugs in ordering get past.
**Undoing it:** drop the replay check; the root's field and `WRITE_ROOT` can stay unused.

## A transaction that fails to apply still stops the journal (TASK-153.14)

A live transaction whose apply fails (a corrupt image's allocator, or a bug) stops the journal: writes fail, reads go on, a remount replays without it. Decided to keep that: it is ext4's errors=remount-ro, and the failure means memory contradicts itself, which writing on would spread. Making apply unable to fail needs a dry run of every mutation against state the transaction's own earlier mutations change, or undo for each kind: a second apply to keep in step, to go on writing on metadata known to be wrong.
**Instead:** check-then-apply under the writer's lock, failing only the transaction.
**Undoing it:** nothing to undo; that is new work.

## A branch's owner is who made it, not a tenant (TASK-153.9, 2026-11-03)

Every branch record now names an owner: a random number chosen by whoever made the branch, unique to that making, and the same in every copy of the record whoever writes it. A fork's host picks its child's owner when it claims the child, and the fork's footer carries it. Two forks naming one child no longer both win, and a fork's cleanup deletes only its own child (TASK-79.82, AC1). Decided: the owner says who made the branch, not which tenant it belongs to. Tenants stay authorized per host by their directory and the VM manager's `CreateStore` (TASK-154).
**Instead:** put the tenant's name in the record, and let a tenant's `OPEN_FORK_FROM` take a point it owns by naming it, which would spare the VM manager a `CreateStore` before a restore on another host. But nothing makes a tenant name mean the same on every host, and it would bring back the tenant-named-branch check that TASK-150.4 removed.
**Undoing it:** a `tenant` field next to `owner`, set by `CreateStore` and passed down from parent to fork, plus a check in `extentd/server.rs`'s `open`.

## A segment's framing ends itself (TASK-153.8, 2026-11-03)

Five bugs came from a framing being ended by whatever came next: a commit's flush, a flush with nothing to commit, `close_now`, closing the branch, or a fork (TASK-41.1, 79.69, 79.99, 92.16, 150.2). Decided: every framing now runs on its own thread, makes its file durable, and finishes itself under the store's state lock. Everything else only waits for it: a commit's flush waits for the segment that commit closed, the uploader for segments whose commits are durable, `close_now` for its own, and forks and closing the branch for all of them. A side effect: the raw file of a segment a commit closed now goes at the first flush or close after both its framing and its commits are durable, not with that commit's own flush.
**Instead:** keep handing each framing to whatever comes after it, as before.
**Undoing it:** revert a38829973.

## Writers declare what's in flight in their keys (TASK-153.7, 2026-11-03)

The collector decides whether an unreferenced object may still be needed from its key. An extent's key names the writer that uploaded it and the segment that will name it. Decided: keep that key as the writer's declaration, and make it exact. Keys always name their segment now (old keys without one are no longer read), and one function, `Pass::may_land`, decides for segments and extents alike.
**Instead:** a separate declaration object or lease field. It would have to land before the objects it covers and be read after the listing, it needs garbage-collecting itself, and it says nothing the key doesn't (`dead-ends/collector-in-flight-declarations.md`).
**Undoing it:** nothing to undo; a declaration object would be added alongside.

## Bucket branches say their format (TASK-141.2, 2026-11-03)

Every branch record now starts with `layout::FORMAT` (1). A reader refuses a newer format with `Error::Format`, naming the branch and the format, before reading or writing anything else of it, and the collector's pass stops without deleting. Framed objects already end with their frame's format (`fxdaxob1`); a newer one is now reported by name instead of as corrupt. The old-lease decoding from before fencing tokens is gone (no migration, AGENTS.md). Store files went to `fxdax extent store 5`, since footers' forks changed.
**Instead:** a format field in every object, footers and checkpoints included. Every reader reaches a branch's objects through its record, so the record is where the check goes.
**Undoing it:** drop `Record::format` and `Record::decode`'s check.

## A panic through a store fences it: fail-stop, usable again once reopened (TASK-153.16, 2026-11-03)

A panic holding a store's lock poisoned it, and every later request of its guest panicked too (TASK-104, 105, 150.20; in `extent_store`, 79.1 and 79.80). Decided: the store is fenced. Every request after the panic fails with `EIO` without panicking, the guest's filesystem stops, and so does its VM. When the connection ends, the store is closed like a writer that left writes uncommitted: it is retired and its followers are cut off. The next open sees it as of its last commit, as after a crash. `crate::fence`: each `ExtentStore` lock can only be taken through the store's fence, and a guard dropped during a panic sets it. The daemon also fences a store when a request, flush or clean of it panics below the locks.
**Instead:** (1) recover the lock's state: nothing proves it consistent across the store's 7 locks and the layers beneath them, even though the three instances happened to panic before changing anything. (2) Fence and reopen in place under the running guest: its filesystem holds the open extents a reopen would lose, and no protocol tells a writer's guest to remount, the way `ESTALE` tells a follower. (3) Turn poison into `EIO` lock by lock: a store whose `open` lock is poisoned would still map from its unpoisoned `dax` lock what the panic left half done.
**Undoing it:** for (1), `StoreMutex::lock` takes `PoisonError::into_inner` and stops checking the fence. The daemon's fencing is in `serve_in_turn`, `Answer::drop` and `flush_behind`.

## The journal's own transaction past its file stops the journal, not the kernel (TASK-60, TASK-139)

`make_room`'s asserts, that the journal's transaction extending its file fits within the old size and a chunk, now stop the journal (`Inner::terminate`) and fail the commit: the guest gets EIO and stays mounted, and replay ends before that transaction. With the default chunk the sizing holds, so the transaction wasn't rebuilt to fit the room it has.
**Instead:** size the own transaction to the room left, which would keep the filesystem writable even then.
**Undoing it:** restore the two `assert!`s in `Journal::make_room`.

## Bases fork on several hosts at once too, as points (TASK-158, 2026-11-03)

A snapshot's point now restores on any number of hosts at once. A fork of anything else, such as a base from `ImportImage`, is a commit on its branch, made by its writer, one host at a time. Decided: `ForkStoreRequest.point` (and `fxdaxctl point SRC DST`) makes a point of any store, once nothing is open, and each host forks a copy of it (`CreateStore` of its branch), as a snapshot's restores do. The base stays writable on its host; what it writes after needs a new point to reach forks. A point of a point is refused (`INVALID_ARGUMENT`). `docs/integrator-guide.md` says how.
**Instead:** bases fork on one host at a time, and the guide says so.
**Undoing it:** drop the field and `ExtentStore::point`'s use from `stores::fork`; nothing else depends on it.

## TASK-150.21 has no guest check: its steps need a staging extent the host doesn't hold (2026-11-03)

guest/sim's `pmd_entry_invalidated_mapped` reaches fs/dax.c:416's WARN by failing the MAP of the staging extent's region after a seal, so that a fault's move into it, a seal shutting the gate, leaves the page a hole. A guest has no fault injection for that MAP, and the only other way, a seal between the restage and its MAP, is a race. Decided: no guest check; the sim's scenario and mutant carry it, and the DAX guest suites run patch 0012's path on a real kernel at every write that says its pages are new.
**Instead:** a module parameter failing the restage's MAP (`fxfs.dax_fail_restage`), and a guest check of the same steps.
**Undoing it:** add the parameter in `fxfs_dax_restage_work` and port the scenario to test_fxfs_guest_checks.rs.

## map_to_unlocked is killed by a scenario, not by seeds (TASK-44.19, 2026-11-03)

3,000 seeds of `interleave` and `failures` missed the mutant (as did every guest DAX suite): it needs two MAPs of one region in flight at once, the nearer ending last, and a seal before the grow worker maps past it again. `guest/sim`'s device now holds a MAP back when a test asks (`sim_device_hold_map`), and `tests/scenarios`' `reaches_at_once` plays the sequence out: the mutant fails it at once, `fxfs_dax_map_to` as it is passes. `zloop.sh`, CI's sim job, now runs `tests/scenarios`' scenarios too, which only `check.sh` and `mutants.sh` ran.
**Instead:** seeded MAP latency in `interleave`, so that seeds find this class without a scenario: it moves every seed and may surface new failures, so it's a task of its own under TASK-149, not done here.
**Undoing it:** drop the scenario and the hold; `zloop.sh` back to the one-run binaries alone.

## 54 stale In Progress tasks triaged: 46 closed, 7 back to To Do (2026-11-02)

Decided with the user: criteria are ours, so a task whose work landed, or whose rest isn't worth doing as written, is closed, and its final summary says what was decided not to do. Unmet criteria stay unchecked. Back to To Do, as real work left that nobody is on: TASK-42.11, 79.82, 91.15, 111.1 (the pgbench decay over five runs, the one task carrying it), 149.2, 149.14, 150.6 (now depending on TASK-153.9); TASK-60 too, waiting on 60.1. Left alone: TASK-158, uncommitted work in worktree t3-9b5982f8. Also closed: TASK-153.6, done by fafefbac7 and 106b6128d, and TASK-153.10, 112.5 and 109.1, about evacuation, which fafefbac7 removed.
**Instead:** keep each open until its criteria are met as written.
**Undoing it:** `backlog task edit <id> -s "To Do"`; each task's notes and summary say what was left.

## Unmerged branches became `archive/` tags, not deleted outright (2026-11-02)

No long-lived branches now (AGENTS.md). Branches already in main went. Unmerged ones, untouched for two days and checked out nowhere, became tags `archive/<branch>`: 50 local, 5 on GitHub, where only main is left. Kept as they were: branches checked out in a worktree or changed in the last two days, as other sessions may be on them.
**Instead:** delete them outright, losing their commits once the reflog expires.
**Undoing it:** `git branch <name> archive/<name>`, then delete the tag.

## Truncate on a full disk: one path zeroes the tail (TASK-150.19)

Rarely, truncated bytes come back on a truly full filesystem. Decided: every non-DAX growth takes one path that zeroes the tail, with one owner of file size (TASK-153.4). Done (2026-11-03): a write raises the size through `txn_grow_size`, and a clone and an allocation zero first. Each writes the block only if it isn't zeros already, so a writeback's growth takes no block it didn't reserve. The cut still zeroes eagerly when it has room, so that what a mapping of the last page shows past the end is zeros; with no room, it no longer matters to the data.
**Instead:** a mid-block shrink could return `NoSpace`, partly reversing TASK-60.
**Undoing it:** the single path stays; only the shrink's error changes.

## Corrupted file data: the host's checksums catch it (TASK-153.11)

A byte damaged in storage, by a bad disk or a bad upload, must make a read fail rather than return the wrong byte. Decided: rely on the host's CRC32C in `extent_frame`, confirm every host read path checks it, and close gaps there. The guest doesn't check.
- Fxfs's own data checksums are read only at journal replay, as in Fuchsia's code, and DAX stores skip Fxfs's write path so have none.
- Pages already in the DAX window are read straight from memory, so nothing can check them there.
- Still uncovered: data on a host's disk before it's sealed, and pages already in memory.

**Instead:** the guest checks too, costing reads on the fast path.
**Undoing it:** adding guest checks later doesn't conflict with anything this does.

## A framed file's raw parts are read whole and checked, as compressed ones are (TASK-153.11)

A part zstd doesn't shrink is stored raw, and reads took its bytes in place, never checking its CRC: a flipped byte in an incompressible extent, local or a fork's base's, read back wrong, and cleaning and seals copied it under a CRC of its own. Decided: a raw part is read whole, checked, and kept with the parts unpacked lately, in the buffer it was read into. Measured (pagein-bench, M-series, 256 MiB, 3 pairs of runs): a raw part's page-in, which reads it whole anyway, +10-30 µs of CPU on about 185 µs; the CRC is 16 µs a MiB. A cleaning pass of 4 KiB raw extents (store-bench `cleaning`, in memory) 15 ms → 23-37 ms for 23 MiB, as each 4 KiB read reads its 1 MiB part. The uploader also checks every file it puts as it is (`read_object`): about 0.5 ms for a 32 MiB segment.
**Instead:** a CRC per 4 KiB block in the table, a format change, so that small reads of raw parts read only their block.
**Undoing it:** `Source::locate` back to `Part::Data` for raw parts of framed files.

## A fork of an extent mapped writably holds it as of the last commit (TASK-153.3)

Today such a fork gets `EBUSY` (TASK-150.7). Decided: the fork holds the extent as of the last commit, as the spec says, so the host half of TASK-153.3 goes ahead. This reaches only a VM process's own raw FORK: production forks use `fork_quiescent`.
**Instead:** the fork also holds the mapped, uncommitted bytes.
**Undoing it:** the fork path in TASK-153.3 copies the mapped bytes as well.

## A device write to an extent not known to be open is taken, not refused (TASK-153.3)

The task's direction had such a write retry rather than reopen its extent. Decided: the store takes it, and nothing records the extent open, ever again: the open extents are those minted since the last seal (`OpenExtents`). Plain writers (multi_write, the journal, layer files) have no retry, where `SealedExtent` is EIO, and DAX's is dax.c's `gate != seals` already.
**Instead:** refuse it in the guest's device, and give every writer a retry.
**Undoing it:** a check in `write_with_opts`, and retries at each writer: `dead-ends/refusing-writes-to-extents-not-known-open.md` has why not.

## Shared blocks of an open extent never go through its region (TASK-153.3)

A clone's blocks in an open extent read `Shared`, and get a range of their own, read-only, never a slice of the extent's region: so a region of an open extent is never mapped read-only. Costs a MAP of their own for reflinked small files' blocks until their extent seals.
**Instead:** keep the checks in `region_open`, `map_to` and `new_slice` that let a reader and a writer share a region (TASK-99, 140, 44.21, 150.11).
**Undoing it:** `fxfs_dax_new_slice`'s one line, and those checks back.

## NoOpenUnderLock.cfg asks what it can now (TASK-153.3)

Its rule, region_open under the region's lock, went with the look it guarded: a reader and a writer don't share a region. The file is back, taking away the rules that keep them apart, and `MapUnderAnOpen.cfg` asks for the race itself, a reader's MAP in flight as a writer opens the region.
**Instead:** put the region's lock back in `region_open`, dead, for the rule to mean what it did.
**Undoing it:** the lock back in dax.c and the rule in the spec.

## 106b6128d's lag under a PTE is fixed on refactor/open-once (TASK-153.3)

A write through a region left a fault's PTE of the page it lags, read stale: found here, verifying the merge, so fixed here, its own commit, with a mutant.
**Instead:** a task of its own on main.
**Undoing it:** revert 157417ad5.
