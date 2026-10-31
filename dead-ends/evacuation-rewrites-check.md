# A guest check that rewrites a page right after the fsync sealing it, to catch evacuation races

**Tried:** 2026-10-25, TASK-101, 85341dafc and fixups, r104, r106, r109.
**Idea:** TASK-101's mutant evacuation_unlocked was killed once in round 5 of r95 and missed after
that. A check writing a page between its evacuation's gathering and MAP should hit the race
reliably.
**Result:** The mutant was missed x10 in each run: r104 at fxfs.dax_delay_us=20000; r106 with each
round writing the page on for 30 ms; r109 with seal_race alongside. Over r95-r109 the mutant was
killed once in ~70 rounds. The check was dropped before landing, by the user's call.
**Why it's dead:** It never stopped a VM with the mutant. The refusal needs a later store to the
page, taken for writable, before the next seal, and no check made one in those runs.
**Reopen if:** A pause point holds the evacuation between gathering and MAP, so the window is forced
rather than hoped for.
