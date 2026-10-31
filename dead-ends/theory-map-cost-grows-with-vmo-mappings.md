# Wrong theory: map_unsafe's cost grows with the mappings of the VMO it maps

**Tried:** 2026-10-23, TASK-92.50, branch r6/zircon-map.
**Idea:** map_unsafe(SPECIFIC_OVERWRITE) rose from 2.5 us per MAP to 105-117 us as the window's
mappings reached ~125k, while the same call from seals' and frees' remaps cost 9-25 us. MAPs of the
open extent map one VMO each time, so an O(n) step walking that VMO's mappings looked likely.
**Result:** The kernel's map and overwrite path is log-time throughout, as upstream's. Each open
extent has a VMO of its own, and a seal remaps its mappings at every commit: what grows is the
window's mapping count. The MAP growth fits a waited STAGE_UNMAP per overwrite of a populated page,
map_unsafe us = -194 + 99 x sends per MAP, r = 0.96.
**Why it's dead:** The premise doesn't hold. A real deviation turned up on the way, a VMO's mappings
kept by pointer, so a range change visited every one (371 -> 1.65 us at 100k mappings, TASK-92.53),
but that was not MAP's growth.
**Reopen if:** Nothing: the Zircon port this measured was deleted by f897ce4a2.
