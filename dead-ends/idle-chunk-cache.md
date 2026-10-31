# An idle-chunk cache to cut writable MAP's page faults

**Tried:** 2026-10-19, TASK-26.18.3.
**Idea:** 70% of the daemon's writable MAP was its VMO's pages faulted in from the host, which
looked like chunks being given back and taken again.
**Result:** It changed nothing.
**Why it's dead:** It wasn't churn: the arena grows, and its pages are never reused in that load.
The cost is first-touch shmem allocation, memcg charging and zeroing. A spare chunk per arena,
faulted in on a thread of its own ahead of use, fixed it (45112ca): daemon mapping 99 -> 40 us,
writable MAP 174 -> 116 us mean.
**Reopen if:** A workload does recycle chunks. The arena this measured was the Zircon port's,
deleted by f897ce4a2.
