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
- **A segment's framing ends itself,** on its own thread, everything else waiting for it. TASK-153.8, 2026-11-03.
- **Writers declare what's in flight in their keys,** one `Pass::may_land` for segments and extents (doc-7). TASK-153.7, 2026-11-03.
- **A branch's owner is who made it, not a tenant.** TASK-153.9, 2026-11-03.
- **A branch record's flags stay the bytes on the wire,** decoded once into a `Kind`; every record and lease change one compare-and-swap. TASK-160.14, 2026-11-03.
- **Bases fork on several hosts at once as points** (`ForkStoreRequest.point`, `fxdaxctl point`; docs/integrator-guide.md). TASK-158, 2026-11-03.
- **A device write to an extent not known to be open is taken,** the open extents being those minted since the last seal (`OpenExtents`). TASK-153.3.
- **Corrupted data is caught by the host's CRCs alone,** not checked in the guest (TASK-153.11); **a framed file's raw parts are read whole and checked**, as compressed ones are (TASK-153.11).
- **The extent store's audit runs in every test build linking it,** for branches of at most 4,096 extents. TASK-160.16, 2026-11-03.
- **A panic through a store fences it:** `EIO` until reopened, as after a crash. TASK-153.16, 2026-11-03.

## The daemon and the VM process

- **What a store may be asked to do is fixed as it opens** (`ExtentStore::access()`): writes need a writer's non-public store, resizes a writer's, forks anything but a follower's. TASK-160.10, 2026-11-03.
- **A follower holds no entry in extentd's store table;** the table keeps a weak reference while anything has the store. TASK-160.8, 2026-11-04.
- **The control socket is 0600, a tenant's 0660 from the moment it appears** (`extentd::bind_given`). TASK-159, 2026-11-04.
- **The daemon tells the VM process the host's `vm.max_map_count`** in OPEN's reply, and the perf gate runs every Firecracker jailed. TASK-172, 2026-11-05.
- **A vCPU's run of failed accesses ends only at a 10 ms gap,** and a failure waits up to 5 ms for its lift. TASK-187, 2026-11-05.
- **One snapshot of extentd, VM processes and guests:** the daemon asks, the guest answers through its device (F_STATS) with its /proc text, `/metrics` asks no VM. TASK-111.4, 2026-11-05.
- **A restore maps its snapshot's window again in one request, `MAP_MANY` (32), eagerly,** not a `MAP` each; mapping lazily on faults was measured and given up (dead-ends/lazy-restore-of-window-mappings.md). TASK-92.14, 2026-11-05.

## DAX

- **The host's copy-on-write protocol:** no flag on `WINDOW` (`args[1]` the private pages); retired numbers not reused; every map read-only. TASK-164.7, 2026-11-04.
- **Private pages are the VM process's memory** (a memfd laid out as the window), not the tenant's arena: a tenant's limit no longer bounds them, `private_max` does. TASK-164.1 and doc-13, 2026-11-04.
- **A PRIVATE refused with ENOMEM stays an EIO,** not a wait. TASK-170, 2026-11-05.
- **A window's mappings carry no K:** a source says what it is (`Source::origin`). TASK-160.11, 2026-11-05.
- **The window's struct pages are made a memory section at a time,** in the driver's `direct_access`; a kernel without FS_DAX takes no window. TASK-92.8, 2026-11-05.
- **What holds a file's data is an operations table of its inode's;** fsync stays super.c's. TASK-160.2, 2026-11-04.
- **The perf gate's thresholds:** doc-13's for tps (`forked-dax` 0.9, failing until TASK-164.6), r632's for the rest. TASK-164.5, 2026-11-05.

## Fxfs

- **Fxfs refuses every version but its own** (`EARLIEST_SUPPORTED_VERSION` is `LATEST_VERSION`); upstream's generic upgrade code stays, unreachable; a refusal is `InvalidVersion`, EINVAL. TASK-141.1, 2026-11-05.
- **TASK-141.1 and 141.3 were rewritten to the no-migrations rule's refusals rather than closed.** TASK-141, 2026-11-05.
- **Once a batch of queued unlinks failed, every change fails,** data's too, as ext4 after a journal abort. TASK-160.23, 2026-11-05.
- **Corruption is EUCLEAN and EBADMSG, not EIO,** in one errno table for both VFSs. TASK-160.21, 2026-11-03.
- **A layer's records are checked as they're read** (+8% a scanned item, +14% with key order; seeks unchanged). TASK-153.12, 2026-11-03.
- **The journal's end is in the root,** set by each sync's barrier; a replay ending before it is refused. TASK-153.13.
- **A transaction that fails to apply still stops the journal** (ext4's errors=remount-ro). TASK-153.14.
- **The journal's own transaction past its file stops the journal, not the kernel.** TASK-139.
- **Truncate on a full disk: every growth takes one path that zeroes the tail.** TASK-150.19.
- **A volume's read-only is derived from its filesystem's, at one place.** TASK-160.26, 2026-11-05.
- **fxdax's additions to Fxfs's upstream files live in child modules,** tests beside them. TASK-160.24, 2026-11-05.

## Tests and CI

- **guest/sim's sweep:** once a call failed, a test's expectations are void and the run goes on to the invariants and its end; a failed writeback loses what changed since the file was durable; every run checks for leaks; TASK-149.18 is guest/sim's, extent_remote's half TASK-149.32. TASK-149.15, 2026-11-06.
- **guest/sim's model of Fxfs is checked by a Rust test building its C** (`crates/fxfs-sim-diff`), its mutants a kind of their own (`Kernel: model`). TASK-149.29, 2026-11-05.
- **Two mutants without killers since copy-on-write DAX:** `task-150.9-resize-moves-the-last-block` equivalent (TASK-149.30 closed without one); `task-79.49-store-fails-on-pins` killed in guest/sim, not by a guest check (TASK-149.31). 2026-11-05.
- **extent_remote's simulation is a CI part of its own,** built with Cargo profile `sim` (release with debug checks). TASK-188, 2026-11-05.
- **CI runs scripts/test.sh, a part a job;** open bugs' failures are listed in known-failures files, not failed on; a nightly `hunt.yml`. TASK-160.27, 2026-11-03.
- **Guest cases are selected by tags;** `manual` for those no suite runs; the nightly runs 50. TASK-160.29, 2026-11-05.
- **Guest test results as data** (`round-K.json`, `run.json`, `summary.json`, uploads to `FXDAX_RESULTS_BUCKET`). TASK-160.31, 2026-11-05.
- **Each guest check runs in a process of its own;** the overrun suite runs the page cache's checks after two made to fail. TASK-160.30, 2026-11-05.
- **A budget per guest check,** a suite's limit their sum plus 30 s. TASK-168, 2026-11-05.

## Repository

- **Unmerged branches became `archive/<branch>` tags, not deleted** (2026-11-02): 50 local, 5 on GitHub, untouched for two days and checked out nowhere. *Instead:* delete them, losing their commits once the reflog expires. *Undoing it:* `git branch <name> archive/<name>`, then delete the tag.
