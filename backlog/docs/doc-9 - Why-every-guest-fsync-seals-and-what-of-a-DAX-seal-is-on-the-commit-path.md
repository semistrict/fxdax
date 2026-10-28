---
id: doc-9
title: 'Why every guest fsync seals, and what of a DAX seal is on the commit path'
type: other
created_date: '2026-10-28 03:23'
updated_date: '2026-10-28 07:16'
---
# Why every guest fsync seals, and what of a DAX seal is on the commit path

2026-10-28, TASK-112.3, answering the sandbox team's round 7b questions (TASK-112). Numbers are
GCE n2-standard-8, nested, `fxfs-guest-pgbench-dax` with a 2 GiB window at ff7961411 (r201-112pg),
and TASK-109's no-DAX baseline.

## 1. Does every fsync need a seal?

Yes, under the device's model, and it should stay so. A guest fsync ends in a barrier (`COMMIT`,
or `WRITE_COMMIT`), and doc-4 has a barrier seal every extent written before it. Write-once
extents sealed by the next barrier were settled in doc-4's design review (2026-10-13); appendable
extents were rejected. What depends on it:

- **Durability is of whole extents.** A commit writes its open extents' data to the store, and the
  host packs sealed extents, in commit order, into segments it uploads and indexes by extent ID
  (doc-4, The host's storage; doc-7). An extent still open after a durable commit would have bytes
  in one segment and later bytes in another, and the index, cleaning and object storage would have
  to version extents.
- **Sharing is by ID.** Forks, snapshots and followers name the extents they share; the host's
  page cache maps one copy of an extent into every VM that reads it. Both are safe only because an
  extent never changes once a commit has named it. An extent left open past a commit is one the
  writer can still change under whoever shares it.
- **Immutability has to be enforced, not trusted.** After the barrier the host makes the window's
  mappings of the sealed pages read-only (`commit_seal_ns`, 58 us a commit, and a write-protect in
  the VMM). Deferring the seal to fork, snapshot or reclaim would not remove that: the committed
  prefix of an open extent would need the same protection, against the same guest, before anything
  else could map it.

Fxfs never rewrites a committed block in place, so a *durable-but-open* staging extent is
conceivable: commit its written prefix, keep the rest writable, seal on fork, snapshot, reclaim or
when it is full. It would save the restage (a new staging extent and its MAP) and the evacuation
after each fsync. It costs a store format whose extents grow across commits, prefix-level
protection of window pages, and seal-on-share in forks, snapshots and followers. Not worth it
while the cheaper items below remain; revisit if they don't close the gap.

## 2. Can the seal's MAPs leave the commit path?

In the guest they already have: `fxfs_dax_sealed` queues the restage (`fxfs_dax_restage_work`)
and the evacuation (`fxfs_dax_evacuate_work`) as background work once the barrier completes. The
round 7 reading of "about three MAPs per seal on the commit path" counts MAPs per seal, not MAPs
the commit waits for.

What a DAX commit itself costs, against no DAX:

| Per commit | DAX | No DAX |
|---|---|---|
| The guest's `COMMIT` round trip (`device_commits_ns`) | ~430 us | ~263 us |
| extentd's commit | 126 us: writing open extents 44, store commit 15, sealing the windows' mappings 58, waiting for uploads 9 | ~25 us |
| The rest: transport, and the VM process's window changes before the reply | ~304 us | ~238 us |

So on the commit path DAX adds ~100 us in extentd, of which writing the data the guest stored
through the window (44 us) replaces the `WRITE`s a commit without DAX follows, and ~66 us of window
changes the reply waits for: its own write-protects, and whatever else the VM process's one
thread had queued ahead of it. Sealing is ~0.43 ms against ~0.26 ms, but both spend ~19 s of a
60 s run sealing: DAX's lower tps is mostly not the commit path.

Off it, the background work costs the transactions directly:

- **Evacuation holds each file's DAX semaphore for writing** while it gathers, MAPs and places the
  file's sealed pages (`fxfs_dax_evacuate_file`): ~650 us each, 32 s of holds a 60 s run
  (`dax_evacuate_hold_ns`), a MAP round trip (~400 us) inside it. Reads, writes and faults of that
  file wait meanwhile: PostgreSQL's WAL is evacuated after nearly every commit.
- **Stores wait for seals**: 10k a run, ~480 us each (`stores_held_by_seals_ns`, 4.9 s).

So the levers, cheapest first:

1. **Evacuate without holding the file for the MAP** (TASK-112.5): gather and place under the
   semaphore, MAP outside it, writers that would map a page of a held range in its own space
   writably backing off (TASK-101). What was tried the night of 2026-10-28, none landed:
   - no evacuation at all: correct, and +11-16% tps while the regions' space lasts (bench VMs
     1513 tps against 1364, above ext4 dax=always on virtio-pmem's 1496), but 667 -> 210 tps at
     2 GiB once it fills: a staging region holds pages of many files, so letting go of ranges
     frees few regions;
   - evacuations batched (16 seals / 20 ms): +3-6%, but data lost under fxfs.dax_delay_us
     (TASK-112.6);
   - the semaphore held for reading through the MAP: writers, the WAL's, still wait;
   - evacuating by dropping the pages instead of mapping them: a range's `mapped` bit means the
     file has data there, so they read as holes.
2. **Order only what must be ordered** in the VM process's window changes: a commit's reply needs
   its own write-protects, not other requests' changes (~66 us at most).
3. **Cheaper seals in extentd**: 58 us a commit, of which finding or making the cached extents
   11.3, the read-only remap 20.3, handing the pages over 8.7, mapping them again 6.4 (stats
   commit-seal-*-ns, 7ac00fa2a). Its write phase went 44 -> 36 us (9048fe2a8).
4. **Cheaper window changes** (TASK-91.16, TASK-109): ~25 us a stage operation nested.
5. **Lazy write-protect** (seal-on-share): only if the above don't close the gap, for the reasons
   in 1.

## 3. What sets the no-DAX ceiling?

Not extentd: its commit is ~25 us of the ~263 us the guest waits. The rest is the round trip under
nested virtualization: the guest's notification exit, the VMM's worker, its socket to extentd and
back, and the interrupt injected. TASK-91.13 (spinning for completions), TASK-91.15 (MMIO exits
per completion) and TASK-91.17 (segment closes inside commits) work on it. Once DAX's extra
~170 us is gone, that round trip is DAX's limit too.
