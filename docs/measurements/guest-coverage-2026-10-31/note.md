# What the DAX checks run of fs/fxfs (TASK-119.14)

GCE r318, `FXDAX_GUEST_KERNEL_BUILD=coverage`, one round each of fxfs-guest-checks-dax, -delay and
-small, merged by `scripts/guest-coverage.py`: `coverage.txt`. dax.c: 81.3% of lines, 60.7% of
branches taken; device.c: 97.5%, 68.4%. super.c's 52.6% is of DAX runs alone: its page cache paths
are the non-DAX checks', which this run leaves out.

Of dax.c's functions never called, all but `fxfs_dax_read` are diagnostics a test asks for
(fxfs.dax_check_moves, dax_verify_seals, dax_shadow). The lines not run are mostly allocation
failures, the window's setup failing, and seals met at a step no check holds one at.
