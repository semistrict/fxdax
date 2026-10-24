# The followers' torture test, an hour each, with and without DAX (TASK-42 AC1, TASK-42.12)

GCE n2-standard-8 (fxdax-test-follow4, us-east4-a), release guest kernel built on the host from
the source (FXDAX_GUEST_KERNEL_BUILD=release), FXDAX_FOLLOW_SECONDS=3600: a writer and two
followers, the protocol's windows widened (apply 200 us, iomap 50 us), every reader's reads
checked by follower_history.

- soak1 (followers-batch1 on main ff83e8bc1): with DAX, OK history (the writer's hour); without
  DAX, both followers STALE and the history WRONG: the workload's buffered writes reached the
  journal at writeback, after later operations (TASK-42.12).
- soak2 (the writer writing back each write before its next operation): OK both ways. Without
  DAX: 170,044 and 170,040 batches, waiting 70 s and 69 s for the fill barrier in all, 13 ms at
  most; 41,239,055 reads checked. With DAX: 176,234 and 176,121 batches, 140 s and 146 s, 93 ms
  and 120 ms at most; 19,078,491 reads checked.

Each run's consoles and its test's log; the histories (some 2 GB a run) are left out.
