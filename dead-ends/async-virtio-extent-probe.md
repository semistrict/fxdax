# Probing virtio_extent asynchronously to take the window's memmap off the boot path

**Tried:** 2026-10-30, TASK-92.8, 1e1e1baf5 (main 3358b1089 plus PROBE_PREFER_ASYNCHRONOUS, a lookup
by tag waiting for probes in progress), r263 against r264. Not pushed.
**Idea:** Building struct pages for the 2 GiB window looked like about a third of a 351 ms kernel
boot, on every fork boot.
**Result:** All suites passed on both. VMM resume to first I/O at init 261.6, 337.2, 261.9, 266.4,
258.0 ms against 258.6, 341.5, 254.0, 252.1, 254.2. 'check access for rdinit' median 0.251 s against
0.272 s over 8 boots each.
**Why it's dead:** No boot gain to show. Whatever the window's memmap costs now, an asynchronous
probe doesn't take it off the path to init and a mount.
**Reopen if:** A boot at loglevel 7 with initcall_debug shows what the probe costs on main today.

**Since:** 2026-11-05, TASK-92.8 measured it (the driver's `window` attribute, GCE r673, jailed): the
probe's memremap_pages of a 2 GiB window took 153-157 ms of a 489-501 ms boot to init (305-333 ms
with no window), of an 8 GiB window 608 ms of 1085. Making the window's struct pages a part at a
time as dax_direct_access reaches them took it off the boot instead (r674: 369-381 ms; 505 ms for
8 GiB). The asynchronous probe stays dead: the cost is there, but deferring the probe didn't move it
off the path to init.
