# Writes waiting until cleaning's pending copies cover the garbage

**Tried:** Analysed and rejected, not built. TASK-79.89, filed by 2026-10-23.
**Idea:** A guest that frees faster than cleaning grows dead space without limit. Making its writes
wait for cleaning would bound the host disk without refusing anything.
**Result:** One pass often can't cover enough: dead space sits in segments at or above CLEAN_BELOW
live, whose live bytes include the uploader's kept frees, and some victims' live data exceeds a pass
(CLEAN_PASS, 64 MiB) or the open segment.
**Why it's dead:** It turns ENOSPC into a hang in exactly those cases. A hard cap past which writes
are refused was chosen (see enospc-on-dead-space.md).
**Reopen if:** Cleaning can always make progress against what a guest frees, for example with moves
outside the open segment, or cleaning sized to what commits free.
