# Followers' READs going through the extent cache

**Tried:** 2026-10-21/22, TASK-42.11. Reverted.
**Idea:** Followers' ~180k READs at ~430 us each bypass the cache and read the store's O_DIRECT
files, while their maps already use the cache. Sharing it would stop them loading the writer's
store.
**Result:** It made the writer's pgbench worse, 1,041 tps.
**Why it's dead:** Page-ins read whole parts, so a small READ through the cache costs a part. The
writer's slowdown was elsewhere anyway: the uploader's close_now holding the state lock.
**Reopen if:** Page-ins can take less than a part for a READ. TASK-92.30 proposes this same change
from reading the code (50-150 us of disk against a 1-2 us copy), so measure it again before doing
it.
