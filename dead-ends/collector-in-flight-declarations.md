# Writers declaring what they have in flight in an object of their own, for the collector

**Tried:** 2026-11-03, TASK-153.7, as a design, against the four bugs it was to rule out (TASK-79.3,
79.18, 79.73, 150.1); not built.
**Idea:** the collector infers which objects no state names may yet be named from their keys, and
four bugs came from that. A writer could instead say what it has in flight: an object, or a field
of its lease, listing the segments and extents it is uploading, which the collector reads and keeps.
**Result:** each of the four came from the key's declaration being incomplete, not from its being a
key: 79.3, an extent's key not naming its segment; 150.1, its branch the minting branch, not the
uploader's; 79.18 and 79.73, readers trusting a key, not the collector. A separate declaration says
nothing the key, made exact, doesn't, and costs more: it must land before the objects it covers,
the collector must read it after listing them, and it needs collecting itself once its writer dies;
one more write per upload, two more orderings of the kind those bugs broke.
**Why it's dead:** the key is written atomically with the object it declares, by its writer, and
names the writer's epoch and the segment that will name it, which is all the collector needs
(`Pass::may_land`). The keys now always name their segments; `Chain::admit` refuses another branch's
extent on its own; rule `extent_keys_name_segments` shows the declaration needed.
**Reopen if:** an object must be kept in flight that its key can't describe, such as one shared by
several branches' segments (merging small objects, doc-7's "Not yet"), or one written before the
writer knows which segment will name it.
