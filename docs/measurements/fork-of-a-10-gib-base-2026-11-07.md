# A fork of a 10 GiB base, before the restructure and after (TASK-165.7)

TASK-165.7 made forks the bucket's alone: a fork is the parent's commit naming the child, and the
child's store, made once the bucket has its record, recovers its state from the bucket as it opens,
O(extents), where the local lineage made a fork a record and layers over the parent's files
(doc-14, Decisions). Here a fork of a 10 GiB base, and the fork's first open, timed on main before
the restructure and after, on one host.

**Run.** GCE r970, `scripts/fork-time-gce.sh fxdax-test-r970-fork RESULTS pre=TREE after=TREE`:
n2-standard-4 (4 vCPUs, 16 GiB), us-east4-a, Cascade Lake, kernel 7.0.0-1011-gcp, both trees
built there in release. *Pre-restructure:* main at aee7cad93, just before TASK-165.6 (local
lineage, layers, the ledger, the index, claims and handles; r914's tree in
restructure-before-after-2026-11-07.md). *After:* main at 3bdec3934, all of TASK-165. For each, a
daemon of its own on a file bucket and a part cache of its own; through its control API, as a host's
control plane's: `fxdaxctl import` of a tar layer of 2,560 files of 4 MiB of random bytes
(10 GiB, which compress no smaller) into a store of 16 GiB, `fxdaxctl sync`, then five times
`fxdaxctl fork` of the base and `fxdaxctl stat` of the fork, which opens it. Each command's wall
time, from fxdaxctl's start to its exit, in the trees' `.tsv`s here.

| | Pre-restructure | After |
|---|---|---|
| fork, the first | 142 ms | 43 ms |
| fork, the four after | 33-35 ms | 27-29 ms |
| the fork's first open (`stat`) | 11-12 ms | 29-31 ms |
| fork and first open | 45-47 ms | 56-60 ms |
| the base's stat | 112 ms | 29 ms |

A fork is a fifth faster after: no claim, record, layers or handle to make; the parent's commit
naming the child, its acknowledgement, the child's record and the child's description. Its first
open is 19 ms slower: the child's state recovered from the bucket, the checkpoint the parent's
writer wrote as the fork's commit was acknowledged, and flattened into the child's own, O(extents)
of the base's, its 2,560 files' data and Fxfs's own, where pre-restructure the child's layers read
the parent's index in memory. A fork and its first open together are 11-13 ms slower, a constant cost for a base this
size, against a fork that no longer ties the child to its parent's directory.

**The import, and what it read back.** The base's import and its uploads took 119 s and then 54 s
pre-restructure (its uploads after it), and 221 s after (its uploads done within it, the sync
0.03 s). The after tree's daemon downloaded 6.8 GB from its bucket during the import (6,800 ranged
GETs) for the 11.0 GB it uploaded, and its part cache held nothing of the base after: the import
makes its store through `Storage` (`crate::import::store`), which opens it with an uploader of no
part cache (`host_store::keeping`), so that the segments the bucket acknowledges are deleted, not
moved into the daemon's cache, and the import's check of the filesystem it made reads them back
from the bucket. Pre-restructure kept them in the store (10.9 GB of stores, nothing downloaded).
On GCS that would be the image's whole size downloaded per import.

| Daemon, its whole run | get | get-range | create | up | down | stores after | cache after |
|---|---|---|---|---|---|---|---|
| pre-restructure | 23 | 3 | 338 | 10.76 GB | 40 KB | 10.9 GB | 0 |
| after | 248 | 6,800 | 225 | 10.98 GB | 6.78 GB | 26 KB | 780 B |
