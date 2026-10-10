TASK-25.9 step B: as 2026-10-10T2146Z, with the writeback cache (FUSE_WRITEBACK_CACHE) offered too.

Iterations: 3. Cases: bench-fs-fxdax-dax bench-fs-fxdax-nodax. Workloads: all.

Finding, against 2026-10-10T2146Z: the writeback cache costs more than it saves. Unlinks fell
from ~3,530 to ~2,560 a second, creates without DAX from 2,193 to 1,973, sequential writes
without DAX from 58 to 43 MiB/s; random writes without DAX rose from ~4,560 to 5,469 and the
build is 2.5% quicker. With it, closing a written file writes it back and sends its times
(fs/fuse/file.c, fuse_flush), more requests than without. Not enabled.
