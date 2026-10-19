# extent_store's mutants left, triaged (TASK-44.1)

Of the 196 that both passes of the sweep left, tests killed 84 (in `crates/extent_store/tests`: store.rs, views.rs, host.rs, lineage.rs, and the Shuttle race). The rest, by why each stands:

- equivalent: 73
- test double: 7
- not testable here: 13
- follow-up: 19

## equivalent

- `dir.rs:110:9` replace HostDir::is_direct -> bool with false: whether extents' files bypass the page cache: performance, the same bytes
- `dir.rs:110:9` replace HostDir::is_direct -> bool with true: whether extents' files bypass the page cache: performance, the same bytes
- `dir.rs:114:12` delete ! in HostDir::file: whether extents' files bypass the page cache: performance, the same bytes
- `dir.rs:114:19` replace && with || in HostDir::file: whether extents' files bypass the page cache: performance, the same bytes
- `dir.rs:184:76` replace | with ^ in <impl Dir for HostDir>::take: LOCK_EX ^ LOCK_NB == LOCK_EX | LOCK_NB: disjoint bits
- `dir.rs:203:75` replace != with == in <impl Dir for HostDir>::let_go: logging only
- `dir.rs:244:9` replace <impl AsRawFd for HostFile>::as_raw_fd -> RawFd with Default::default(): whether extents' files bypass the page cache: performance, the same bytes
- `dir.rs:292:5` replace bypass_page_cache -> bool with false: whether extents' files bypass the page cache: performance, the same bytes
- `dir.rs:292:5` replace bypass_page_cache -> bool with true: whether extents' files bypass the page cache: performance, the same bytes
- `dir.rs:297:19` replace >= with < in bypass_page_cache: whether extents' files bypass the page cache: performance, the same bytes
- `dir.rs:297:24` replace && with || in bypass_page_cache: whether extents' files bypass the page cache: performance, the same bytes
- `dir.rs:297:64` replace | with & in bypass_page_cache: whether extents' files bypass the page cache: performance, the same bytes
- `dir.rs:297:64` replace | with ^ in bypass_page_cache: whether extents' files bypass the page cache: performance, the same bytes
- `dir.rs:297:82` replace == with != in bypass_page_cache: whether extents' files bypass the page cache: performance, the same bytes
- `dir.rs:303:54` replace == with != in bypass_page_cache: whether extents' files bypass the page cache: performance, the same bytes
- `framed.rs:48:28` replace * with / in read_aligned: reads more aligned bytes than needed, of which the same are kept
- `framed.rs:50:37` replace - with + in read_aligned: reads more aligned bytes than needed, of which the same are kept
- `framed.rs:173:25` replace + with * in push_data: parts not merged: more parts, the same bytes
- `framed.rs:214:20` replace == with != in Part<F>::is_empty: Part::is_empty is used by no reader of parts
- `framed.rs:214:9` replace Part<F>::is_empty -> bool with false: Part::is_empty is used by no reader of parts
- `framed.rs:214:9` replace Part<F>::is_empty -> bool with true: Part::is_empty is used by no reader of parts
- `framed.rs:292:20` replace += with *= in Unpacked::put: the clock of the parts kept unpacked, ticking in get too: eviction order alike
- `host.rs:43:55` replace > with >= in lineage: keeps dead weak entries in the open stores' list: memory only
- `host.rs:125:41` replace && with || in delete_host_after: another error for a handle half deleted; the open stores' list, weak entries
- `host.rs:146:55` replace != with == in delete_host_after: another error for a handle half deleted; the open stores' list, weak entries
- `layers.rs:47:9` replace <impl std::fmt::Debug for Layer>::fmt -> std::fmt::Result with Ok(Default::default()): Debug output only
- `layers.rs:220:9` replace <impl std::fmt::Debug for Extents>::fmt -> std::fmt::Result with Ok(Default::default()): Debug output only
- `layers.rs:363:24` replace > with < in Extents::freeze: how deep layers chain, and which layer a fork shares: memory, the same extents
- `layers.rs:363:24` replace > with == in Extents::freeze: how deep layers chain, and which layer a fork shares: memory, the same extents
- `layers.rs:363:24` replace > with >= in Extents::freeze: how deep layers chain, and which layer a fork shares: memory, the same extents
- `layers.rs:374:9` replace Extents::rebase with (): how deep layers chain, and which layer a fork shares: memory, the same extents
- `layout.rs:270:32` replace match guard piece.extent_offset == 0 && piece.len == entry.len with true in Checkpoint::table: one piece covering its extent: from offset 0 if and only if as long as it
- `layout.rs:270:57` replace && with || in Checkpoint::table: one piece covering its extent: from offset 0 if and only if as long as it
- `layout.rs:386:47` replace ^ with & in hash: another seed for the records' hash, the same in writing and checking
- `layout.rs:386:47` replace ^ with | in hash: another seed for the records' hash, the same in writing and checking
- `lineage.rs:361:24` replace >= with < in Lineage<D>::segment_file: the cap on open segment files: resource use
- `lineage.rs:362:60` replace > with < in Lineage<D>::segment_file: the cap on open segment files: resource use
- `lineage.rs:362:60` replace > with >= in Lineage<D>::segment_file: the cap on open segment files: resource use
- `lineage.rs:439:31` replace += with *= in Lineage<D>::open_held: tokens tell openings of one branch apart, which never overlap here
- `lineage.rs:539:9` replace Lineage<D>::latest_layer_of -> Option<Arc<Layer>> with None: which layer a new fork shares with its siblings: memory, the same extents
- `lineage.rs:541:29` delete ! in Lineage<D>::latest_layer_of: which layer a new fork shares with its siblings: memory, the same extents
- `lineage.rs:543:49` replace == with != in Lineage<D>::latest_layer_of: which layer a new fork shares with its siblings: memory, the same extents
- `lineage.rs:571:32` replace match guard entry.open == Some(token) with true in Lineage<D>::closed: tokens tell openings of one branch apart, which never overlap here
- `lineage.rs:626:9` replace Lineage<D>::shared -> bool with false: cleaning a segment another branch shares copies its data needlessly; the segment stays for its holders
- `lineage.rs:629:13` replace || with && in Lineage<D>::shared: cleaning a segment another branch shares copies its data needlessly; the segment stays for its holders
- `lineage.rs:648:31` replace != with == in Lineage<D>::remove: logging, or a removal of what is gone
- `lineage.rs:662:24` replace match guard branch == first with true in Branches::live: the error message for a deleted store
- `lineage.rs:855:21` replace match guard err.kind() == std::io::ErrorKind::NotFound with true in remove_branch_index: logging, or a removal of what is gone
- `store.rs:722:49` replace && with || in place_of: one piece covering its extent: from offset 0 if and only if as long as it
- `store.rs:1230:24` replace += with *= in Store<D>::close_idle_files: caps on open files: resource use; files reopen as read
- `store.rs:1233:26` replace -= with += in Store<D>::close_idle_files: caps on open files: resource use; files reopen as read
- `store.rs:1255:34` replace >= with < in Store<D>::own_file: caps on open files: resource use; files reopen as read
- `store.rs:1256:76` replace > with < in Store<D>::own_file: caps on open files: resource use; files reopen as read
- `store.rs:1256:76` replace > with == in Store<D>::own_file: caps on open files: resource use; files reopen as read
- `store.rs:1256:76` replace > with >= in Store<D>::own_file: caps on open files: resource use; files reopen as read
- `store.rs:1359:16` replace > with >= in Store<D>::finish_write: grows by 0 when equal
- `store.rs:1375:36` replace == with != in Store<D>::finish_write: a wake also made by written() when the last write in progress ends
- `store.rs:1396:32` replace == with != in Store<D>::abandon_write: a wake also made by written() when the last write in progress ends
- `store.rs:1485:29` replace > with >= in Store<D>::parts_locked: locating 0 bytes of an extent's own file adds nothing
- `store.rs:1659:55` delete field durable from struct Uploads expression in Store<D>::upload: the durable epoch an uploader starts with gates only later commits, past it either way
- `store.rs:1770:57` replace > with >= in Store<D>::commit_locked: what commits retain of sealed extents for views: memory, the same refusals
- `store.rs:1835:34` replace -= with += in Store<D>::commit_locked: caps on open files: resource use; files reopen as read
- `store.rs:1835:34` replace -= with /= in Store<D>::commit_locked: caps on open files: resource use; files reopen as read
- `store.rs:1946:13` delete match arm (Data::Remote, _) in Store<D>::seal: unreachable: adopted extents are never open, and a large extent written whole is in its own file from its first write
- `store.rs:1954:42` replace match guard !large with true in Store<D>::seal: unreachable: adopted extents are never open, and a large extent written whole is in its own file from its first write
- `store.rs:2170:16` delete ! in Store<D>::close_now: logging only
- `store.rs:2263:20` delete ! in Store<D>::flush: logging only
- `store.rs:2292:40` replace > with >= in Store<D>::clean: an empty segment's live bytes (0) are never under a fraction of 0
- `store.rs:2563:30` replace < with <= in paint: a span of 0 bytes, or two empty spans side by side, which painting never makes
- `store.rs:2592:17` delete match arm (None, None) in paint: a span of 0 bytes, or two empty spans side by side, which painting never makes
- `views.rs:176:9` replace <impl std::fmt::Debug for View>::fmt -> std::fmt::Result with Ok(Default::default()): Debug output only
- `views.rs:224:9` replace Keeping::oldest -> Option<u64> with Some(0): what commits retain of sealed extents for views: memory, the same refusals
- `views.rs:323:31` replace < with <= in Views::publish: views are never as of a commit published again: their epochs are always older

## test double

- `mem.rs:146:9` replace MemDir::io -> Io with Default::default(): MemDir is the tests' directory: its I/O counters, failure and crash model are what tests use, not what they check
- `mem.rs:152:9` replace MemDir::has_failed -> bool with true: MemDir is the tests' directory: its I/O counters, failure and crash model are what tests use, not what they check
- `mem.rs:188:39` replace < with <= in MemDir::crash: MemDir is the tests' directory: its I/O counters, failure and crash model are what tests use, not what they check
- `mem.rs:271:9` replace <impl Dir for MemDir>::barrier -> io::Result<()> with Ok(()): MemDir is the tests' directory: its I/O counters, failure and crash model are what tests use, not what they check
- `mem.rs:278:20` replace += with *= in <impl File for MemFile>::read_at: MemDir is the tests' directory: its I/O counters, failure and crash model are what tests use, not what they check
- `mem.rs:290:23` replace += with *= in <impl File for MemFile>::write_at: MemDir is the tests' directory: its I/O counters, failure and crash model are what tests use, not what they check
- `mem.rs:293:29` replace < with <= in <impl File for MemFile>::write_at: MemDir is the tests' directory: its I/O counters, failure and crash model are what tests use, not what they check

## not testable here

- `dir.rs:175:9` replace <impl Dir for HostDir>::sync_dir -> io::Result<()> with Ok(()): durability system calls: what they keep shows only after a power loss; MemDir models crashes for the store's logic
- `dir.rs:210:9` replace <impl Dir for HostDir>::barrier -> io::Result<()> with Ok(()): durability system calls: what they keep shows only after a power loss; MemDir models crashes for the store's logic
- `dir.rs:213:83` replace < with > in <impl Dir for HostDir>::barrier: durability system calls: what they keep shows only after a power loss; MemDir models crashes for the store's logic
- `dir.rs:216:20` delete ! in <impl Dir for HostDir>::barrier: durability system calls: what they keep shows only after a power loss; MemDir models crashes for the store's logic
- `dir.rs:258:28` replace += with *= in <impl File for HostFile>::read_at: a short or interrupted read of a regular file, which needs fault injection
- `dir.rs:260:29` replace match guard err.kind() == io::ErrorKind::Interrupted with false in <impl File for HostFile>::read_at: a short or interrupted read of a regular file, which needs fault injection
- `dir.rs:260:29` replace match guard err.kind() == io::ErrorKind::Interrupted with true in <impl File for HostFile>::read_at: a short or interrupted read of a regular file, which needs fault injection
- `dir.rs:260:40` replace == with != in <impl File for HostFile>::read_at: a short or interrupted read of a regular file, which needs fault injection
- `dir.rs:276:9` replace <impl File for HostFile>::sync -> io::Result<()> with Ok(()): durability system calls: what they keep shows only after a power loss; MemDir models crashes for the store's logic
- `store.rs:523:5` replace not_found -> bool with true: which I/O errors read as a missing file: needs fault injection
- `store.rs:769:25` replace match guard not_found(&err) with true in close_left_open: closing segments left open after a crash, on I/O errors: needs fault injection
- `store.rs:774:29` replace match guard !not_found(&err) with false in close_left_open: closing segments left open after a crash, on I/O errors: needs fault injection
- `store.rs:784:25` replace match guard !not_found(&err) with false in close_left_open: closing segments left open after a crash, on I/O errors: needs fault injection

## follow-up

- `host.rs:157:5` replace remove -> std::io::Result<()> with Ok(()): what removing gives when another deletion removed it first: needs two deletions racing
- `host.rs:158:21` replace match guard err.kind() == std::io::ErrorKind::NotFound with false in remove: what removing gives when another deletion removed it first: needs two deletions racing
- `host.rs:158:21` replace match guard err.kind() == std::io::ErrorKind::NotFound with true in remove: what removing gives when another deletion removed it first: needs two deletions racing
- `host.rs:158:32` replace == with != in remove: what removing gives when another deletion removed it first: needs two deletions racing
- `lineage.rs:74:23` replace -= with += in dropped: the lineage's bookkeeping of files forks drop: not reached by the model (tests/lineage.rs); to look into
- `lineage.rs:74:23` replace -= with /= in dropped: the lineage's bookkeeping of files forks drop: not reached by the model (tests/lineage.rs); to look into
- `lineage.rs:79:38` replace > with >= in dropped: the lineage's bookkeeping of files forks drop: not reached by the model (tests/lineage.rs); to look into
- `lineage.rs:401:19` replace == with != in Lineage<D>::hold: the directory held while a branch is open: another process must wait, which takes a test of 10 s
- `lineage.rs:690:52` replace && with || in Branches::let_go_of_inherited: the lineage's bookkeeping of files forks drop: not reached by the model (tests/lineage.rs); to look into
- `lineage.rs:758:20` replace == with != in Branches::removable: the lineage's bookkeeping of files forks drop: not reached by the model (tests/lineage.rs); to look into
- `lineage.rs:826:16` delete ! in build_layers_of: layers rebuilt at opening for forks of forks: the model reopens lineages; to look into
- `lineage.rs:873:25` replace match guard err.kind() == std::io::ErrorKind::AlreadyExists with false in create_branch_dirs: directories left by a creation that stopped partway: needs a crash between them
- `lineage.rs:873:25` replace match guard err.kind() == std::io::ErrorKind::AlreadyExists with true in create_branch_dirs: directories left by a creation that stopped partway: needs a crash between them
- `lineage.rs:873:36` replace == with != in create_branch_dirs: directories left by a creation that stopped partway: needs a crash between them
- `store.rs:820:13` replace || with && in rebuild: rebuilding a branch from footers that do not follow on: a crash test of segments out of order
- `store.rs:1838:31` replace match guard last > state.checkpointed with true in Store<D>::commit_locked: a closed segment dead before a checkpoint covers it: removed a commit later; needs a test of when
- `store.rs:1838:36` replace > with >= in Store<D>::commit_locked: a closed segment dead before a checkpoint covers it: removed a commit later; needs a test of when
- `store.rs:2316:31` replace match guard error.downcast_ref::<NoExtent>().is_some() with false in Store<D>::clean: an extent freed while cleaning copies it: needs a race (Shuttle)
- `store.rs:2316:31` replace match guard error.downcast_ref::<NoExtent>().is_some() with true in Store<D>::clean: an extent freed while cleaning copies it: needs a race (Shuttle)

