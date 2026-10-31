# DAX inodes' write(2) going through Fxfs's write path, reads and mmap stores kept on the window

**Tried:** 2026-10-28, TASK-118, branch exp/118-write-fxfs (97eb31c2d, fxfs.dax_write_fxfs), GCE
r205.
**Idea:** No-DAX commits cost 250 us against DAX's 483 us: a write(2) and fsync is a WRITE and a
COMMIT, with nothing to seal, restage or evacuate. Each write would copy into a buffer and call
`fxfs_write`, after dropping the chunks it touches, and a heuristic could pick the fsync-per-write
streams if tps reached no-DAX's.
**Result:** pgbench-dax 609/261/235/236/311 tps against 1172/1085/1022/983/983 off, same host and
tree: ~25%. window_maps_range ~1.4M per 60 s run, window_maps_ns ~35 s, window_unmaps_ns ~13 s. Data
stayed correct under fxfs.dax_delay_us=200; only mechanism asserts failed.
**Why it's dead:** Each write(2) drops its 2 MiB chunk, so reads map it all again.
**Reopen if:** The write path invalidates only the written pages and maps them again from Fxfs's new
blocks. TASK-118 notes that is close to what the window write path already does.
