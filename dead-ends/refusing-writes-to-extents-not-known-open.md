# A device write to an extent not known to be open refused, for the writer to retry

**Tried:** Analysed, not built. 2026-11-01, TASK-153.3, the direction its task set out.
**Idea:** What made an extent read as open after sealed was a write to one a seal had forgotten: if
the guest's device refuses such writes with `SealedExtent`, the writer retries with new blocks, and
nothing reopens.
**Result:** Only `dax_map_write` retries `SealedExtent`. `multi_write`, journal and layer-file writes
have no retry: there it is EIO, and a compaction's shuts the journal down (TASK-91.1). All of them
take extents minted before a seal may come, so every seal in flight would refuse some.
**Why it's dead:** Refusing isn't what stops the reopening. The write is harmless: the store takes it,
as no seal sealed anything of the extent there, and the next seal seals it. What did harm was
recording it open. With no write recording anything (`OpenExtents`), the retry DAX needs already
exists: dax.c's `gate != seals` moves the blocks again.
**Reopen if:** A store refuses writes to extents minted before a seal, which it doesn't know of.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): `dax_map_write` and dax.c's gate
are gone; DAX's data reaches the device as WRITEs at writeback, as other writes' do.
