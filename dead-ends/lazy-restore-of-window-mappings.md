# Restoring a snapshot's window mappings lazily, on the guest's faults

**Tried:** 2026-11-05, TASK-92.14, measured rather than built: the restore test
`extentd::tests::snapshots_map_their_windows_again_in_one_request` on Linux (Docker, aarch64).
**Idea:** a restore maps every recorded mapping before the VM resumes. Recording them in the
window's map without installing any page, and letting the guest's first access to each page
fault it in, would make the restore itself cost almost nothing.
**Result:** with the whole map sent in one `MAP_MANY`, restoring 1,280 mappings (1,024 one-page
extents, 256 runs of zeros) took 14-23 ms, of which the VM process's installs of the pages already
in memory were 3.6-5.5 ms. The rest is the daemon's work per mapping (resolving each extent's
cached source, placing it in the window's map, recording the change), which a lazy restore does
just the same. Each page left out instead costs a fault after resume: a userfaultfd exit, a round
trip to the daemon and the same install, tens of microseconds each, against a few for an install
batched with the others.
**Why it's dead:** it saves a quarter of the restore and pays it back several times over in
faults on every page the guest touches, and the pages it would skip are only those already in
memory (a restore on another host, whose daemon has none of them, faults them in either way).
**Reopen if:** restores of zygotes whose guests touch little of their window after resume turn out
to dominate, or the installs become most of a `MAP_MANY`'s time.
