# Open questions

## The nightly needs GCP credentials in GitHub (2026-11-02)

`.github/workflows/nightly.yml` never ran: its YAML didn't parse (fixed in a1e079306). Now it parses, it needs the repository's `GCP_WORKLOAD_IDENTITY_PROVIDER` and `GCP_SERVICE_ACCOUNT` secrets and its `FXDAX_GCE_PROJECT` and `FXDAX_NIGHTLY_BUCKET` variables, none of which is set. Granting GitHub access to the GCP project is yours.
**Recommended:** a workload identity pool for `semistrict/fxdax` and a service account allowed to make and delete GCE instances and write the nightly bucket.
**Until then:** each nightly fails at its auth step; TASK-69's runs stay manual.

# Decisions

Decisions made without you, still yours to confirm or overturn. Each is a line or two here; what else was possible and how to undo it are in the task named, under "Decision moved here from OPEN_QUESTIONS.md" in its notes, or in the doc named. Tell an agent which you confirm or overturn and its line leaves the file.

## Storage: the bucket, the WAL and the store

- **Acknowledged segments leave the store** for the host's part cache, recorded in the index (`Record::Acknowledged`); a store with no uploader keeps everything; a follower whose extent the collector reclaimed gets `ESTALE`; a `memory:///` bucket loses what it acknowledged when its process exits; offline forks attach an uploader first (`ExtentStore::kept_in_bucket`). TASK-165.8, 2026-11-05.
- **Forks, points and deletion are the bucket's alone:** a child's store is made once its record exists and takes the child's state then (`remote::take_branch`, O(extents)); a failed fork's leftovers stand as an orphan for `fxdax-gc --delete-branch`; a fork needs an uploader; `ancestry` and what's attached or public go into the fork's directory; an image's bucket is `<image>.bucket`, beside it, never removed; a directory with no description is no store. TASK-165.7, 2026-11-05.
- **Local disk as the bucket's WAL and cache, how:** WAL segments stamped as they close; a commit's local durability a log beside the open segment; a diverged WAL refuses (`ESTALE`) until `fxdaxctl take`; capacity is branch state set by commits; one spending rule; the uploader moves into extent_store; the part cache kept across restarts; the collector the only cleaner; file buckets on `LocalFileSystem` under `flock`; steps 7 and 8 in one batch. TASK-165 and doc-14, 2026-11-04.
- **The WAL is placed after the branch by commits, not segment links** (TASK-165.2; doc-14 says so), and **a fork's segment alone after the branch's last commit is taken as landed** (`Wal::rebase`'s `rposition`, TASK-165.5). 2026-11-05.
- **The part cache keeps no index file:** rebuilt from its files, which describe themselves, every read checked. TASK-165.4, 2026-11-05.
- **One replay, capacity by commits:** a resize is the next commit's; a store restates its capacity with its next commit; a batch's footer has a store's shape. TASK-165.3, 2026-11-05.
- **Every store's description names its bucket** (local format 6, now 7); images keep their branches in a file bucket of their own (since TASK-165.7 beside the image, `<image>.bucket`, not inside it); stores on a host of fxdax's system name the host's bucket; the in-process collector runs only for an image's own bucket. TASK-165.6, 2026-11-05.
- **The collector:** ways to extents start at their seals unless the newest checkpoint is the lease holder's, and writers take their checkpoint's moves first (TASK-175, with TASK-177 to 180's restarts); it recovers a branch once by each origin naming it (TASK-173); it keeps every record (TASK-174); a deletion fences the branch at the end it recovered to (TASK-183). doc-7 has the rules. 2026-11-05.
- **specs/bucket models TASK-183's fix as a rule (`deletionsFence`) and TASK-178's check with no `Rules` field;** TASK-177's half of `newest_checkpoint` has its own mutation. TASK-183, 2026-11-05.
- **A bucket request times out by its progress, not as a whole:** fxdax's own HTTP client (`extent_remote::client`, reqwest built from `object_store`'s options) fails one once no bytes of it moved for 30 s, connects within 5 s, and sets no deadline on the whole; objects still go in one conditional PUT, `object_store`'s multipart having no create-if-absent. TASK-205, 2026-11-06.
- **A segment's framing ends itself,** on its own thread, everything else waiting for it. TASK-153.8, 2026-11-03.
- **Writers declare what's in flight in their keys,** one `Pass::may_land` for segments and extents (doc-7). TASK-153.7, 2026-11-03.
- **A branch's owner is who made it, not a tenant.** TASK-153.9, 2026-11-03.
- **A branch record's flags stay the bytes on the wire,** decoded once into a `Kind`; every record and lease change one compare-and-swap. TASK-160.14, 2026-11-03.
- **Bases fork on several hosts at once as points** (`ForkStoreRequest.point`, `fxdaxctl point`; docs/integrator-guide.md). TASK-158, 2026-11-03.
- **A device write to an extent not known to be open is taken,** the open extents being those minted since the last seal (`OpenExtents`). TASK-153.3.
- **Corrupted data is caught by the host's CRCs alone,** not checked in the guest (TASK-153.11); **a framed file's raw parts are read whole and checked**, as compressed ones are (TASK-153.11).
- **The extent store's audit runs in every test build linking it,** for branches of at most 4,096 extents. TASK-160.16, 2026-11-03.
- **A panic through a store fences it:** `EIO` until reopened, as after a crash. TASK-153.16, 2026-11-03.
- **doc-12's "durable within 50 ms" becomes a 50 ms median and a separate bound on what a daemon crash may lose,** which tests check; the worst tail is perf work. The user's decision, 2026-11-06 (TASK-221); the TASK-165.9 agent applies it to doc-12 and its test.

## The daemon and the VM process

- **What a store may be asked to do is fixed as it opens** (`ExtentStore::access()`): writes need a writer's non-public store, resizes a writer's, forks anything but a follower's. TASK-160.10, 2026-11-03.
- **A follower holds no entry in extentd's store table;** the table keeps a weak reference while anything has the store. TASK-160.8, 2026-11-04.
- **The control socket is 0600, a tenant's 0660 from the moment it appears** (`extentd::bind_given`). TASK-159, 2026-11-04.
- **The daemon tells the VM process the host's `vm.max_map_count`** in OPEN's reply, and the perf gate runs every Firecracker jailed. TASK-172, 2026-11-05.
- **A vCPU's run of failed accesses ends only at a 10 ms gap,** and a failure waits up to 5 ms for its lift. TASK-187, 2026-11-05.
- **One snapshot of extentd, VM processes and guests:** the daemon asks, the guest answers through its device (F_STATS) with its /proc text, `/metrics` asks no VM. TASK-111.4, 2026-11-05.
- **A compromised VM process harms no tenant beside it** (TASK-149.17's harness): arenas sealed against shrinking and 0600, reads and writes copied 1 MiB at a time, roots capped at 4 KiB on the wire (`wire::MAX_ROOT`), each fixed at the narrowest place. TASK-149.17, 2026-11-06.
- **A restore maps its snapshot's window again in one request, `MAP_MANY` (32), eagerly,** not a `MAP` each; mapping lazily on faults was measured and given up (dead-ends/lazy-restore-of-window-mappings.md). TASK-92.14, 2026-11-05.
- **A VM process, its daemon and its device each say a version and refuse another:** `wire::VERSION` in OPEN's `args[4]` and its reply's, which a fingerprint test ties to the wire; every request required (`BUILD`, `TRANSFER`); `VIRTIO_EXTENT_VERSION` first in the device's configuration, its features no negotiation (a driver takes every one offered), a snapshot of another protocol refused; the driver's test sets its version by `virtio_extent.version`. The control API's "v1 only grows" is left as it is. TASK-141.3, 2026-11-05.
- **SIGTERM drains the daemon:** VMs still connected are cut off at once; within `FXDAX_EXTENTD_DRAIN` (60 s), exit 1 past it; done once no uploader runs, each connection's last commits uploaded with its store still open (TASK-192 says why not at close). TASK-141.4, 2026-11-05.
- **A restore opens a store already at its path only if it holds what the point does** (root and extents: the snapshotted store unwritten since, or a fork an earlier restore left unwritten); any other is refused with `EEXIST` and left as it is, not forked afresh over, which would throw away what its VM wrote. TASK-194, 2026-11-06.
- **The Go client is a module of its own, `github.com/semistrict/fxdax/go`, versioned by fxdax's commits** (pseudo-versions; no tags cut): its package `extentd/v1` (`extentdv1`) generated by buf from the .proto, no protoc, whose version every generated file would name; buf pinned in `go/generate.sh` (`go run`), the generators by go.mod's `tool`s, protoc-gen-go so with the runtime it generates for. `NewClient` wraps the stubs: the socket dialed whatever its path, the health service, a failed call's errno found by `errors.Is`/`errors.As` (`syscall.Errno`, Linux's numbers). *Instead:* the module at the repo root (go.mod there), tags `go/vX.Y.Z`, or stubs alone. *Undoing it:* move or delete `go/`, drop `go_package` from the .proto. TASK-219, 2026-11-06.
- **A tenant's in-flight data is bounded per tenant too:** its connections share one room, configurable. The user's decision, 2026-11-06 (TASK-149.35); another agent implements it.

## DAX

- **The host's copy-on-write protocol:** no flag on `WINDOW` (`args[1]` the private pages); retired numbers not reused; every map read-only. TASK-164.7, 2026-11-04.
- **Private pages are the VM process's memory** (a memfd laid out as the window), not the tenant's arena: a tenant's limit no longer bounds them, `private_max` does. TASK-164.1 and doc-13, 2026-11-04.
- **A PRIVATE refused with ENOMEM stays an EIO,** not a wait. TASK-170, 2026-11-05. The host's refusal, that is: one the guest found no memory to ask for fails ENOMEM (`fxfs_device_private` tells them apart). TASK-44.15, 2026-11-06.
- **A writeback Fxfs had no memory for keeps its pages dirty,** their room reserved again, failing ENOMEM without setting the file's error; an I/O error's pages are lost as TASK-149.15 has it; an eviction gives them up after a second of tries. doc-13's writeback, step 4. The page cache's writeback likewise keeps such folios dirty (TASK-212, 2026-11-06). *Instead:* the file marked failed, every later fsync failing, which loses data for a transient error. TASK-207, 2026-11-06.
- **A window's mappings carry no K:** a source says what it is (`Source::origin`). TASK-160.11, 2026-11-05.
- **The window's struct pages are made a memory section at a time,** in the driver's `direct_access`; a kernel without FS_DAX takes no window. TASK-92.8, 2026-11-05.
- **What holds a file's data is an operations table of its inode's;** fsync stays super.c's. TASK-160.2, 2026-11-04.
- **A `PRIVATE`'s pages are one change of the window, made by the VM process as a run** (one mmap, none where nothing is installed); **a write(2) following on from its file's last makes 16 to 64 pages ahead private**; **a rebind maps a range's runs in one `MAP_MANY`, a range at a time, the files dirtied longest ago first.** TASK-164.10, 2026-11-05.
- **A write by any thread not the guest's, on a window that copies on write, resolves as a load** and restricts the page for KVM's worker: no worker makes a page private, a guest making its pages private before it stores. TASK-191, 2026-11-06.
- **The perf gate's thresholds:** doc-13's for `dax`, r632's for the rest (TASK-164.5, 2026-11-05); **`forked-dax` 0.8, the user's decision** that the forked pgbench at ~0.84 of no DAX (r650: 0.836, 0.854) is doc-13's one host round trip a first store worth paying, its further cuts TASK-164.11's (TASK-164.6, 2026-11-06).
- **A window's `private_max` is half its pages by default,** not a quarter: each DAX VM reserves twice as much against its tenant's arena (1 GiB of a 2 GiB window), and holds up to that much private memory. A forked pgbench's PRIVATEs fall from 309,608 to 108,006 and forked-dax rises from 0.83 to 0.91 (GCE r802, r804); the density gate passes (r805: 2,256 MiB a VM added, from r794's 2,097; 0.82 of no DAX's). *Undoing it:* `private_max` in vendor/firecracker's extent device back to `pages / 4`, or `dax_private_max` set per VM. TASK-164.11, 2026-11-06.
- **TASK-164 and TASK-164.11 are done with TASK-164.11.1 and TASK-164.11.2 open,** later work: forked-dax reached 0.912 (GCE r807) with fewer PRIVATEs, made before the inode's lock, not with cheaper ones, which they are about (the VMM's request path; a PRIVATE's run without an mmap). *Undoing it:* TASK-164.11 back to In Progress until they are done. 2026-11-07.
- **The dax-gate's MAP probe is a map's mean, not a request's,** ceiling 80 us (half as much again as r794's 53): since TASK-164.10 a MAP request holds ~38 maps, ~2 ms, past the request's 560 us ceiling from writable DAX's r365. *Undoing it:* the request's mean back, with a ceiling of copy-on-write DAX's requests. TASK-164.6, 2026-11-06.

## Fxfs

- **Extended attributes are written through,** each set or removal a transaction of its own (ENOSPC when full, removal always works); `write_inode` keeps a failed inode dirty, so every fsync reports it. TASK-199, 2026-11-06.
- **A failed thaw leaves the filesystem frozen and the next resumes at the step that failed;** a read-only superblock's freeze reaches Fxfs through wrapped `freeze_super`/`thaw_super` (an emergency sysrq thaw bypasses them). TASK-200, TASK-202, 2026-11-06.
- **The driver's queues close under their locks before a reset,** then the stats work is flushed, then the device resets. TASK-201, 2026-11-06.
- **Fxfs refuses every version but its own** (`EARLIEST_SUPPORTED_VERSION` is `LATEST_VERSION`); upstream's generic upgrade code stays, unreachable; a refusal is `InvalidVersion`, EINVAL. TASK-141.1, 2026-11-05.
- **Fxfs format 62: `UpdateBorrowed` records where its transaction's journal usage counts from and what its reservation pays**, so replay finds the metadata space borrowed as the filesystem had it; format-61 stores are refused, as any older format is. TASK-218, 2026-11-06.
- **TASK-141.1 and 141.3 were rewritten to the no-migrations rule's refusals rather than closed.** TASK-141, 2026-11-05.
- **Once a batch of queued unlinks failed, every change fails,** data's too, as ext4 after a journal abort. TASK-160.23, 2026-11-05.
- **Corruption is EUCLEAN and EBADMSG, not EIO,** in one errno table for both VFSs. TASK-160.21, 2026-11-03.
- **A layer's records are checked as they're read** (+8% a scanned item, +14% with key order; seeks unchanged). TASK-153.12, 2026-11-03.
- **The journal's end is in the root,** set by each sync's barrier; a replay ending before it is refused. TASK-153.13.
- **A transaction that fails to apply still stops the journal** (ext4's errors=remount-ro). TASK-153.14.
- **The journal's own transaction past its file stops the journal, not the kernel.** TASK-139.
- **What a dropped shared allocation left in its extent is journaled at the next sync** (`AllocatorMutation::Reach`, added last to V60 without a version bump, as `Reference` was), so a mount sizes the extent from it. TASK-213, 2026-11-06.
- **Truncate on a full disk: every growth takes one path that zeroes the tail.** TASK-150.19.
- **A volume's read-only is derived from its filesystem's, at one place.** TASK-160.26, 2026-11-05.
- **fxdax's additions to Fxfs's upstream files live in child modules,** tests beside them. TASK-160.24, 2026-11-05.
- **After a power cut a staging extent may stay uncounted, up to 2 MiB a volume, until it dies:** no device request for extents' lengths, as not worth a protocol change now. The user's decision, 2026-11-06 (TASK-214, Low).
- **The staging extent's ranges in flight stay counted twice for a while** (alive and uncommitted), erring safe: ENOSPC a little early. The user's decision, 2026-11-06 (TASK-211).

## Tests and CI

- **guest/sim's sweep:** once a call failed, a test's expectations are void and the run goes on to the invariants and its end; a failed writeback loses what changed since the file was durable; every run checks for leaks; TASK-149.18 is guest/sim's, extent_remote's half TASK-149.32. TASK-149.15, 2026-11-06.
- **guest/sim's model of Fxfs is checked by a Rust test building its C** (`crates/fxfs-sim-diff`), its mutants a kind of their own (`Kernel: model`). TASK-149.29, 2026-11-05.
- **Two mutants without killers since copy-on-write DAX:** `task-150.9-resize-moves-the-last-block` equivalent (TASK-149.30 closed without one); `task-79.49-store-fails-on-pins` killed in guest/sim, not by a guest check (TASK-149.31). 2026-11-05.
- **extent_remote's simulation is a CI part of its own,** built with Cargo profile `sim` (release with debug checks). TASK-188, 2026-11-05.
- **CI runs scripts/test.sh, a part a job;** open bugs' failures are listed in known-failures files, not failed on; a nightly `hunt.yml`. TASK-160.27, 2026-11-03.
- **Guest cases are selected by tags;** `manual` for those no suite runs; the nightly runs 50. TASK-160.29, 2026-11-05.
- **Guest test results as data** (`round-K.json`, `run.json`, `summary.json`, uploads to `FXDAX_RESULTS_BUCKET`). TASK-160.31, 2026-11-05.
- **fsx against ext4 in one guest:** the model is the oracle of bytes and errnos, ext4 held to it as Fxfs is; ext4 says which times a step changes, compared (`differential::ALLOWED` empty); a clone on ext4 is a copy of what the model's VFS rules say it takes; failures shrunk in the guest, replayed once from a boot; -faults-dax in soak-sanitized alone. TASK-44.15, 2026-11-06.
- **xfstests' known failures less generic/225, 564 and 798,** which pass with DAX and without since copy-on-write DAX. TASK-44.15, 2026-11-06.
- **Each guest check runs in a process of its own;** the overrun suite runs the page cache's checks after two made to fail. TASK-160.30, 2026-11-05.
- **A budget per guest check,** a suite's limit their sum plus 30 s. TASK-168, 2026-11-05.
- **fxfs-guest-soak:** a soak round is a runner round, its state kept by the run outside the results (/var/tmp, not tmpfs /tmp); stores are checked by a guest on each, not the host's fsck (which would take the branch's lease); its daemons are built with failpoints; a store whose upload the failpoint failed may see EIO (remote.rs stops its uploader for good); no bucket latency on file buckets until TASK-208; the cache's growth is reported, not failed on. TASK-67, 2026-11-06.
- **The GCE quota stays at 32.** The user's decision, 2026-11-06.

## Repository

- **Unmerged branches became `archive/<branch>` tags, not deleted** (2026-11-02): 50 local, 5 on GitHub, untouched for two days and checked out nowhere. *Instead:* delete them, losing their commits once the reflog expires. *Undoing it:* `git branch <name> archive/<name>`, then delete the tag.
- **`FileType::default_perm` and its test `default_permissions` deleted,** dead since TASK-96.1. The user's decision, 2026-11-06.
