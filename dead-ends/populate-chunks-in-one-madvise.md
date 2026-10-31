# Populating each chunk in one madvise to cut writable MAP's page faults

**Tried:** 2026-10-19, TASK-26.18.3.
**Idea:** 70% of the daemon's writable MAP was its VMO's pages faulted in page by page. One call per
chunk would avoid the per-fault overhead.
**Result:** It only moved the cost.
**Why it's dead:** The time is in the host's allocation, memcg charge and zeroing of each page, not
in the faults taking it. Doing that work on the MAP's path costs the same however it's called. It
was moved off the path instead, by a spare chunk faulted in ahead of use (45112ca).
**Reopen if:** Nothing in this form.
