---
id: doc-9
title: 'Why every guest fsync seals, and what of a DAX seal is on the commit path'
type: other
created_date: '2026-10-28 03:23'
updated_date: '2026-10-28 03:23'
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

What the commit still pays for them is on the host. A DAX commit takes the guest ~430 us
(`device_commits_ns`), against ~263 us without DAX. Of it:

| Part | Per commit |
|---|---|
| extentd's commit | 126 us: writing open extents 44, store commit 15, sealing the windows' mappings 58, waiting for uploads 9 |
| VMM window changes in the reply's stream | 10.5 stage operations at ~25 us, ~262 us of one thread's time |

The VM process applies the daemon's window changes on one thread, in the order the daemon sent
them, before the replies that follow them (that order is what makes a commit's write-protects
precede its reply). The background evacuation and restage MAPs go through the same stream, so a
commit's reply waits behind every change queued ahead of it, not only its own.

So the levers, cheapest first:

1. **Order only what must be ordered.** A commit's reply needs its own write-protects applied, not
   the evacuation and restage changes of earlier commits. Those could go on a second stream, or
   the VM process could apply changes to disjoint window ranges in parallel. (Protocol and VMM
   change.)
2. **Fewer evacuation MAPs** (TASK-109.1): most written pages are mapped twice, once in the staging
   region and once in their range's own space after the seal.
3. **Cheaper window changes** (TASK-91.16, TASK-109): 25 us a stage operation nested, after
   1cd30919's ~20% cut.
4. **Lazy write-protect** (seal-on-share): only if 1-3 don't close the gap, for the reasons in 1.

## 3. What sets the no-DAX ceiling?

Not extentd: its commit is ~25 us of the ~263 us the guest waits. The rest is the round trip under
nested virtualization: the guest's notification exit, the VMM's worker, its socket to extentd and
back, and the interrupt injected. TASK-91.13 (spinning for completions), TASK-91.15 (MMIO exits
per completion) and TASK-91.17 (segment closes inside commits) work on it. Once DAX's extra
~170 us is gone, that round trip is DAX's limit too.
