// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * Paths no seed of tests/interleave takes, each played out exactly: memory that ends part way
 * through a write, a write from the pages it writes, a writer killed, allocations and device
 * requests that fail. And copy-on-write DAX's own (doc-13): rebinding and writeback against
 * stores, truncating and punching dirty pages, and a crash between writeback's bytes and its
 * commit.
 *
 *   scenarios [NAME]
 *
 * runs the scenario NAME, or, with none, lists them: the process is one scenario's, as sim_init
 * takes one seed, and a sweep of allocations fails each in turn within it.
 */

#include <sim/sim.h>
#include <linux/virtio_extent.h>

#define WINDOW_LEN (64UL << 20)
/* A chunk's data and more, which only a range of its own maps. */
#define BIG_LEN (3UL << 20)
#define PAUSE_READ_MAPPED BIT(4)
#define PAUSE_WRITE_MAPPED BIT(5)
#define PAUSE_WRITEBACK BIT(8)
#define PAUSE_FAULTED BIT(9)
/* A chunk's range, the window's whole ranges, at a chunk's alignment. */
#define CHUNK_LEN (2UL << 20)
/*
 * The device's private pages, few, so that the budget, past seven eighths of them, has the worker
 * write back and rebind while tasks store.
 */
#define COW_PRIVATE_MAX 64

#define check(cond, ...) do { if (!(cond)) sim_panic(__VA_ARGS__); } while (0)

static struct sim_mount *mount;

static void fill(char *buf, size_t len, unsigned char seed)
{
	size_t i;

	for (i = 0; i < len; i++)
		buf[i] = seed + i % 251;
}

/*
 * That read(2), and Fxfs's extents once written back, hold `expected`, now and once what the
 * writes before left dead is freed, as a range mapping blocks where they were then reads zeros.
 */
static void check_file(const char *what, struct file *file, const char *expected, size_t len)
{
	char *got = kmalloc(len, GFP_KERNEL);
	struct inode *inode = file_inode(file);
	int round;
	ssize_t read;

	for (round = 0; round < 2; round++) {
		check(!sim_fsync(file), "%s: fsync", what);
		read = sim_read(file, 0, got, len);
		check(read == len, "%s: read %zd of %zu", what, read, len);
		check(!memcmp(got, expected, len), "%s: read(2) differs, round %d", what, round);
		fxfs_sim_read(mount->info->fs, inode->i_ino, 0, got, len);
		check(!memcmp(got, expected, len), "%s: Fxfs's extents differ, round %d", what, round);
		for (int seals = 0; seals < 8; seals++)
			check(!sim_fsync(file), "%s: fsync", what);
		msleep(10);
	}
	kfree(got);
}

static struct file *new_file(const char *bytes, size_t len)
{
	struct file *file = sim_file_new(mount);
	ssize_t done = sim_write(file, 0, bytes, len);

	check(done == len, "a new file's write: %zd", done);
	return file;
}

/* An address `readable` bytes before the end of a mapping of a file of `bytes`. */
static unsigned long stops_after(const char *bytes, size_t readable)
{
	size_t len = round_up(readable, PAGE_SIZE);

	return sim_mmap(new_file(bytes, len), 0, len) + len - readable;
}

/* That a write of a page from memory that ends 100 bytes on keeps the rest of the page. */
static void short_copy(void)
{
	static char old[2 * PAGE_SIZE], source[PAGE_SIZE], expected[2 * PAGE_SIZE];
	struct file *file;
	ssize_t done;

	memset(old, 'A', sizeof(old));
	memset(source, 'B', sizeof(source));
	file = new_file(old, sizeof(old));
	check(!sim_fsync(file), "fsync");
	done = sim_write_user(file, 0, stops_after(source, 100), PAGE_SIZE);
	check(done == 100, "a write from memory ending after 100 bytes: %zd", done);
	memcpy(expected, old, sizeof(old));
	memset(expected, 'B', 100);
	check_file("past the copy", file, expected, sizeof(expected));
}

/* As short_copy, over more pages than the source reaches, in a file with a range of its own. */
static void short_copy_pages(void)
{
	static char old[BIG_LEN], source[3 * PAGE_SIZE];
	char *expected = kmalloc(BIG_LEN, GFP_KERNEL);
	const size_t readable = PAGE_SIZE + 2000;
	struct file *file;
	ssize_t done;

	memset(old, 'A', sizeof(old));
	fill(source, sizeof(source), 1);
	file = new_file(old, sizeof(old));
	check(!sim_fsync(file), "fsync");
	done = sim_write_user(file, PAGE_SIZE, stops_after(source, readable), 3 * PAGE_SIZE);
	check(done == readable, "a write from memory ending part way: %zd", done);
	memcpy(expected, old, BIG_LEN);
	memcpy(expected + PAGE_SIZE, source + round_up(readable, PAGE_SIZE) - readable, readable);
	check_file("past the copy", file, expected, BIG_LEN);
	kfree(expected);
}

/* That a write of a page from a mapping of that page leaves it as it was. */
static void same_page(void)
{
	static char bytes[3 * PAGE_SIZE], expected[3 * PAGE_SIZE];
	struct file *file;
	unsigned long addr;
	ssize_t done;

	fill(bytes, sizeof(bytes), 1);
	memcpy(expected, bytes, sizeof(bytes));
	file = new_file(bytes, sizeof(bytes));
	check(!sim_fsync(file), "fsync");
	addr = sim_mmap(file, 0, sizeof(bytes));
	done = sim_write_user(file, 0, addr, PAGE_SIZE);
	check(done == PAGE_SIZE, "a write from its own page: %zd", done);
	check_file("the page written from itself", file, expected, sizeof(expected));
	/* Each page from the one after it, which the page before's copy mustn't take. */
	done = sim_write_user(file, 0, addr + PAGE_SIZE, 2 * PAGE_SIZE);
	check(done == 2 * PAGE_SIZE, "a write from the next pages: %zd", done);
	memmove(expected, expected + PAGE_SIZE, 2 * PAGE_SIZE);
	check_file("the pages written from those after", file, expected, sizeof(expected));
	/* In one page, apart: the iomap of the write unmapped what it was to copy, each time. */
	check(!sim_fsync(file), "fsync");
	done = sim_write_user(file, 100, addr + 2000, 100);
	check(done == 100, "a write from a page of its target: %zd", done);
	memmove(expected + 100, expected + 2000, 100);
	check_file("the bytes written from their page", file, expected, sizeof(expected));
}

/*
 * That an append stopped short leaves the file, and Fxfs's once written back, as long as it wrote,
 * and the next append's bytes where they go. On writable DAX, Fxfs moved the last block as the file
 * was cut to what was written, and the next append's bytes went where it was.
 */
static void append_stopped_short_in_a_page(void)
{
	static char source[2 * PAGE_SIZE], expected[2 * PAGE_SIZE];
	const size_t readable = 5000, after = 100;
	struct file *file = sim_file_new(mount);
	ssize_t done;

	fill(source, sizeof(source), 1);
	memcpy(expected, source + sizeof(source) - readable, readable);
	done = sim_write_user(file, 0, stops_after(source, readable), 2 * PAGE_SIZE);
	check(done == readable, "an append from memory ending part way: %zd", done);
	check(!sim_fsync(file), "fsync");
	check(fxfs_sim_size(mount->info->fs, file_inode(file)->i_ino) == readable,
	      "Fxfs's size, after an append stopped short: %llu",
	      fxfs_sim_size(mount->info->fs, file_inode(file)->i_ino));
	memset(expected + readable, 'x', after);
	done = sim_write(file, readable, expected + readable, after);
	check(done == after, "the next append: %zd", done);
	check(!sim_truncate(file, sizeof(expected)), "growing the file");
	check_file("the appends", file, expected, sizeof(expected));
}

/* That bytes stored past the file's end read as zeros once a write past it grows the file. */
static void store_past_end_then_written(void)
{
	static char bytes[100], expected[2 * PAGE_SIZE + 8], got[2 * PAGE_SIZE + 8];
	struct file *file;
	unsigned long addr;
	ssize_t done;

	memset(bytes, 'D', sizeof(bytes));
	file = new_file(bytes, sizeof(bytes));
	addr = sim_mmap(file, 0, PAGE_SIZE);
	check(!sim_user_store(addr + 1000, "X", 1), "a store past the end");
	memcpy(expected, bytes, sizeof(bytes));
	memset(expected + 2 * PAGE_SIZE, 'E', 8);
	done = sim_write(file, 2 * PAGE_SIZE, expected + 2 * PAGE_SIZE, 8);
	check(done == 8, "a write past the end: %zd", done);
	done = sim_read(file, 0, got, sizeof(got));
	check(done == sizeof(got), "the read: %zd", done);
	check(!memcmp(got, expected, sizeof(got)), "past the old end: byte 1000 is %#x", got[1000]);
}

/* That a write past the end that copies nothing leaves the file as long as it was. */
static void write_past_end_copies_nothing(void)
{
	static char bytes[100], got[100];
	struct file *file;
	ssize_t done;

	memset(bytes, 'D', sizeof(bytes));
	file = new_file(bytes, sizeof(bytes));
	done = sim_write_user(file, 2 * PAGE_SIZE, 1UL << 20, 8);
	check(done == -EFAULT, "a write from no memory: %zd", done);
	check(i_size_read(file_inode(file)) == sizeof(bytes), "the file is %lld bytes long",
	      i_size_read(file_inode(file)));
	check(!sim_fsync(file), "fsync");
	check(fxfs_sim_size(mount->info->fs, file_inode(file)->i_ino) == sizeof(bytes),
	      "in Fxfs: %llu", fxfs_sim_size(mount->info->fs, file_inode(file)->i_ino));
	done = sim_read(file, 0, got, sizeof(got));
	check(done == sizeof(got) && !memcmp(got, bytes, sizeof(got)), "the read: %zd", done);
}

/* A file's ranges gone, as a reflink's, to be mapped again. */
static void drop_ranges(struct file *file)
{
	struct inode *inode = file_inode(file);
	int err;

	filemap_invalidate_lock(inode->i_mapping);
	err = fxfs_dax_changed(mount->info->dax, mount->info->dax_volume, inode, inode->i_ino, 0,
			       MAX_LFS_FILESIZE);
	filemap_invalidate_unlock(inode->i_mapping);
	check(!err, "dropping its ranges: %d", err);
}

static struct file *killed_file;
static char killed_bytes[4 * PAGE_SIZE];

static void write_killed_file(void *arg)
{
	ssize_t done = sim_write(killed_file, 0, killed_bytes, sizeof(killed_bytes));

	check(done == -EINTR, "the write, its task killed: %zd", done);
}

/*
 * That a writer killed past its iomap, before its copy, leaves the file as it was: dax_iomap_iter
 * stops at the signal, the pages it made private and dirty holding what they held, which writeback
 * writes as they were. On writable DAX, a move committed before its bytes left zeros where the
 * file's data was (TASK-153.5).
 */
static void write_killed(void)
{
	static char bytes[sizeof(killed_bytes)];
	struct task_struct *writer;
	char value[24];

	fill(bytes, sizeof(bytes), 1);
	fill(killed_bytes, sizeof(killed_bytes), 7);
	killed_file = new_file(bytes, sizeof(bytes));
	check(!sim_fsync(killed_file), "fsync");
	snprintf(value, sizeof(value), "%lu", file_inode(killed_file)->i_ino);
	sim_param_set("dax_pause_ino", value);
	snprintf(value, sizeof(value), "%lu", PAUSE_WRITE_MAPPED);
	sim_param_set("dax_pause", value);
	writer = sim_spawn("writer", write_killed_file, NULL);
	while (!(sim_param_read("dax_paused") & PAUSE_WRITE_MAPPED))
		msleep(1);
	writer->killed = true;
	sim_param_set("dax_pause", "0");
	msleep(100);
	check(i_size_read(file_inode(killed_file)) == sizeof(bytes), "the file is %lld bytes long",
	      i_size_read(file_inode(killed_file)));
	check_file("the write, its task killed", killed_file, bytes, sizeof(bytes));
}

/* That a failed UNMAP's pages, kept from use for good, don't stop the filesystem unmounting. */
static void unmap_failed(void)
{
	static char big[BIG_LEN];
	struct file *file;

	memset(big, 'a', sizeof(big));
	file = new_file(big, sizeof(big));
	check(!sim_fsync(file), "fsync");
	sim_device_fail(VIRTIO_EXTENT_T_UNMAP, 0, 1);
	check(!sim_truncate(file, 0), "the truncate");
	check(sim_device_failed() == 1, "no UNMAP failed");
	sim_file_evict(file);
	sim_mount_free(mount);
	mount = NULL;
}

static char sweep_bytes[PAGE_SIZE * 64];

/* Goes round allocating with the `skip`th allocation failing, for each until none is left. */
static void alloc_sweep(const char *what, void (*op)(struct file *, unsigned long, int), bool mapped)
{
	char *bytes = sweep_bytes;
	const size_t len = sizeof(sweep_bytes);
	struct file *file;
	unsigned long addr = 0;
	u64 skip, failed;

	fill(bytes, len, 1);
	file = new_file(bytes, len);
	if (mapped)
		addr = sim_mmap(file, 0, len);
	for (skip = 0;; skip++) {
		/* Pieces of the file, each in its turn: every map of them has more than one. */
		for (int page = 0; page < 8; page += 2)
			check(sim_write(file, page * PAGE_SIZE, bytes + page * PAGE_SIZE,
					PAGE_SIZE) == PAGE_SIZE, "a piece");
		check(!sim_fsync(file), "fsync");
		drop_ranges(file);
		failed = sim_allocs_failed();
		sim_alloc_fail(skip, 1);
		op(file, addr, skip);
		sim_alloc_fail(0, 0);
		if (sim_allocs_failed() == failed)
			break;
		check_file(what, file, bytes, len);
	}
	check(skip > 3, "%s: %llu allocations alone", what, skip);
}

static void read_op(struct file *file, unsigned long addr, int skip)
{
	static char got[PAGE_SIZE * 8];

	sim_read(file, 0, got, sizeof(got));
}

static void write_op(struct file *file, unsigned long addr, int skip)
{
	/* What it holds, so that a write that fails part way leaves the file as it was. */
	char same[PAGE_SIZE];

	memcpy(same, sweep_bytes + 3 * PAGE_SIZE, sizeof(same));
	sim_write(file, 3 * PAGE_SIZE, same, sizeof(same));
}

static void load_op(struct file *file, unsigned long addr, int skip)
{
	char byte;

	sim_user_load(addr + 3 * PAGE_SIZE, &byte, 1);
}

/* That whichever allocation fails, nothing is freed twice, and the file reads as it did. */
static void allocations_failed(void)
{
	alloc_sweep("a read", read_op, false);
	alloc_sweep("a write", write_op, false);
	alloc_sweep("a load", load_op, true);
}

static void wait_for(struct task_struct *task)
{
	while (!task->done)
		msleep(1);
}

/*
 * Copy-on-write DAX (doc-13): what a file's pages hold, each a round in its first 8 bytes, which
 * the task storing to it alone changes, and the rest of the page as it was written first.
 */
#define COW_PAGES 128
static struct file *cow_target;
static unsigned long cow_addr;
static char cow_expected[COW_PAGES * PAGE_SIZE];
static u64 cow_round[COW_PAGES];
static bool cow_stop;

/* A file of `pages` pages, written, written back and sealed: each page round 0. */
static struct file *cow_file(size_t pages)
{
	struct file *file;
	size_t page;

	fill(cow_expected, pages * PAGE_SIZE, 5);
	for (page = 0; page < pages; page++) {
		memset(cow_expected + page * PAGE_SIZE, 0, sizeof(u64));
		cow_round[page] = 0;
	}
	file = new_file(cow_expected, pages * PAGE_SIZE);
	check(!sim_fsync(file), "the file's fsync");
	return file;
}

/* Stores `round` to `page` of cow_target: through its mapping, or by write(2). */
static void cow_put(u64 page, u64 round, bool mapped)
{
	loff_t at = page * PAGE_SIZE;

	if (mapped)
		check(!sim_user_store(cow_addr + at, &round, sizeof(round)),
		      "a store to page %llu", page);
	else
		check(sim_write(cow_target, at, &round, sizeof(round)) == sizeof(round),
		      "a write to page %llu", page);
	memcpy(cow_expected + at, &round, sizeof(round));
	WRITE_ONCE(cow_round[page], round);
}

/* That `page` holds its last round, through its mapping or by read(2). */
static void cow_check(u64 page, bool mapped, const char *what)
{
	loff_t at = page * PAGE_SIZE;
	u64 got = ~0ULL;

	if (mapped)
		check(!sim_user_load(cow_addr + at, &got, sizeof(got)), "a load of page %llu", page);
	else
		check(sim_read(cow_target, at, &got, sizeof(got)) == sizeof(got),
		      "a read of page %llu", page);
	check(got == cow_round[page], "%s: page %llu reads round %llu, its last store round %llu: a store was lost",
	      what, page, got, cow_round[page]);
}

/*
 * That read(2), and Fxfs once written back, hold every page's last round and the rest of the
 * page as written; that no room for dirty pages is left reserved, nor given back twice; and that
 * nothing asked to map an open extent.
 */
static void cow_check_file(const char *what, size_t len)
{
	struct inode *inode = file_inode(cow_target);

	check(!sim_fsync(cow_target), "%s: fsync", what);
	check_file(what, cow_target, cow_expected, len);
	check(!fxfs_sim_reserved(mount->info->fs, inode->i_ino),
	      "%s: %llu bytes of dirty pages' room reserved, with none dirty", what,
	      fxfs_sim_reserved(mount->info->fs, inode->i_ino));
	/* The model's Fxfs says open of what writeback wrote until its seal: none is mapped. */
	check(!sim_device_open_maps_refused(), "%s: %llu maps of open extents refused (EPERM)", what,
	      sim_device_open_maps_refused());
}

/* Stores to the first pages, each checked to hold its last first, till cow_stop. */
static void cow_storer(void *arg)
{
	u64 pages = (uintptr_t)arg, page, round;

	for (round = 1; !READ_ONCE(cow_stop); round++) {
		for (page = 0; page < pages; page++) {
			cow_check(page, round % 2, "before the next store");
			cow_put(page, round, (round + page) % 3);
		}
		msleep(1);
	}
}

/* Writes each page past `first`, in rounds, so that private pages run out and are rebound. */
static void cow_pusher(void *arg)
{
	u64 first = (uintptr_t)arg, page, round;

	for (round = 1; round <= 4; round++) {
		for (page = first; page < COW_PAGES; page++)
			cow_put(page, round, false);
	}
}

static void cow_syncer(void *arg)
{
	while (!READ_ONCE(cow_stop)) {
		check(!sim_fsync(cow_target), "the syncer's fsync");
		msleep(2);
	}
}

/*
 * That a rebind loses no store: stores go on to some pages, through a mapping and by write(2),
 * while writes to the rest, twice private_max, have the worker write back and rebind clean pages,
 * a syncer cleaning the stored pages between rounds. A rebind under the file's semaphore shared,
 * or of a page in either set, MAPs an extent's page over a private page holding a store, which
 * reads its round before.
 */
static void cow_rebind_against_stores(void)
{
	struct task_struct *storer, *pusher, *syncer;

	cow_target = cow_file(COW_PAGES);
	cow_addr = sim_mmap(cow_target, 0, COW_PAGES * PAGE_SIZE);
	storer = sim_spawn("storer", cow_storer, (void *)16UL);
	syncer = sim_spawn("syncer", cow_syncer, NULL);
	pusher = sim_spawn("pusher", cow_pusher, (void *)16UL);
	wait_for(pusher);
	WRITE_ONCE(cow_stop, true);
	wait_for(storer);
	wait_for(syncer);
	check(sim_device_private_freed(), "no private page was rebound");
	check(sim_device_private_most() <= COW_PRIVATE_MAX, "%llu private pages",
	      sim_device_private_most());
	cow_check_file("after the rebinds", COW_PAGES * PAGE_SIZE);
}

/*
 * That an fsync keeps every store that completed before it began: stores go on, through a
 * mapping and by write(2), while each fsync, once it returns, finds in Fxfs at least the round
 * each page had before it began. A swap of the dirty set after the write-protect, or under the
 * semaphore shared, leaves a store in neither set: the fsync after it writes nothing of it.
 */
static void cow_writeback_against_stores(void)
{
	const u64 pages = 32;
	struct task_struct *storer;
	u64 floor[32], got, page;
	int fsyncs;

	cow_target = cow_file(pages);
	cow_addr = sim_mmap(cow_target, 0, pages * PAGE_SIZE);
	storer = sim_spawn("storer", cow_storer, (void *)(uintptr_t)pages);
	for (fsyncs = 0; fsyncs < 40; fsyncs++) {
		for (page = 0; page < pages; page++)
			floor[page] = READ_ONCE(cow_round[page]);
		check(!sim_fsync(cow_target), "fsync %d", fsyncs);
		for (page = 0; page < pages; page++) {
			fxfs_sim_read(mount->info->fs, file_inode(cow_target)->i_ino,
				      page * PAGE_SIZE, &got, sizeof(got));
			check(got >= floor[page] && got <= READ_ONCE(cow_round[page]) + 1,
			      "fsync %d returned, and Fxfs's page %llu holds round %llu: the store of round %llu completed before it began",
			      fsyncs, page, got, floor[page]);
		}
		msleep(sim_random() % 3);
	}
	WRITE_ONCE(cow_stop, true);
	wait_for(storer);
	cow_check_file("after the fsyncs", pages * PAGE_SIZE);
}

/* Stores `round` to each page from `first` to `last`, through the mapping and by write(2). */
static void cow_put_pages(u64 first, u64 last, u64 round)
{
	u64 page;

	for (page = first; page <= last; page++)
		cow_put(page, round, page % 2);
}

/*
 * That truncating dirty pages, private, drops them from both sets and frees them, releasing their
 * room: the file reads zeros there once it grows again, as Fxfs does, past what writeback wrote;
 * and the pages written back and stored to again, clean or not, likewise.
 */
static void cow_truncate_dirty(void)
{
	const u64 pages = 32, size = 8 * PAGE_SIZE + 100;
	static const char zeros[PAGE_SIZE - 100];
	char tail[PAGE_SIZE - 100];

	cow_target = cow_file(pages);
	cow_addr = sim_mmap(cow_target, 0, pages * PAGE_SIZE);
	cow_put_pages(0, pages - 1, 1);
	check(!sim_fsync(cow_target), "fsync");
	cow_put_pages(16, pages - 1, 2);
	cow_put_pages(20, 23, 3);
	check(!sim_truncate(cow_target, size), "the truncate");
	check(sim_device_private() <= 9, "%llu pages private past the truncate's end",
	      sim_device_private());
	check(!sim_user_load(cow_addr + size, tail, sizeof(tail)), "a load past the end, in its page");
	check(!memcmp(tail, zeros, sizeof(tail)), "the last page past the end isn't zeros");
	memset(cow_expected + size, 0, pages * PAGE_SIZE - size);
	cow_check_file("past the truncate", size);
	check(fxfs_sim_size(mount->info->fs, file_inode(cow_target)->i_ino) == size,
	      "Fxfs's size, past the truncate: %llu",
	      fxfs_sim_size(mount->info->fs, file_inode(cow_target)->i_ino));

	check(!sim_truncate(cow_target, pages * PAGE_SIZE), "growing the file");
	cow_check_file("grown again", pages * PAGE_SIZE);
	cow_put_pages(9, pages - 1, 4);
	cow_check_file("stored to again", pages * PAGE_SIZE);
}

/* Writes and punches the pages from `first` on, again and again, as a syncer writes back. */
static void cow_puncher(void *arg)
{
	u64 first = (uintptr_t)arg, round, page;

	for (round = 1; round <= 20; round++) {
		cow_put_pages(first, first + 7, round);
		check(!sim_punch(cow_target, first * PAGE_SIZE, (first + 8) * PAGE_SIZE),
		      "punch %llu", round);
		for (page = first; page < first + 8; page++) {
			memset(cow_expected + page * PAGE_SIZE, 0, PAGE_SIZE);
			WRITE_ONCE(cow_round[page], 0);
		}
		msleep(sim_random() % 3);
	}
}

/*
 * That a punch of dirty pages, with writeback under way and stores beside it, leaves a hole: no
 * writeback that copied the pages before it commits them after it.
 */
static void cow_punch_dirty(void)
{
	const u64 pages = 16;
	struct task_struct *storer, *syncer, *puncher;

	cow_target = cow_file(pages);
	cow_addr = sim_mmap(cow_target, 0, pages * PAGE_SIZE);
	storer = sim_spawn("storer", cow_storer, (void *)8UL);
	syncer = sim_spawn("syncer", cow_syncer, NULL);
	puncher = sim_spawn("puncher", cow_puncher, (void *)8UL);
	wait_for(puncher);
	WRITE_ONCE(cow_stop, true);
	wait_for(storer);
	wait_for(syncer);
	cow_check_file("after the punches", pages * PAGE_SIZE);
}

static void cow_store_during_commit(void *arg)
{
	cow_put(3, 2, true);
}

static void cow_fsync(void *arg)
{
	check(!sim_fsync(cow_target), "the held fsync");
}

/*
 * That a crash between writeback's bytes and its commit shows the file as it was, whole pages, and
 * the commit as writeback copied them: a commit before its bytes would show what its extent held
 * unwritten. A store meanwhile, which takes no lock a writeback holds, goes to the private page,
 * dirty again, which the fsync's writeback of its dirty pages (fxfs_dax_write_back) writes after.
 */
static void cow_crash_between_bytes_and_commit(void)
{
	const u64 pages = 8;
	static char old[8 * PAGE_SIZE], got[8 * PAGE_SIZE];
	struct task_struct *syncer, *storer;
	u64 ino;

	cow_target = cow_file(pages);
	ino = file_inode(cow_target)->i_ino;
	memcpy(old, cow_expected, sizeof(old));
	cow_addr = sim_mmap(cow_target, 0, pages * PAGE_SIZE);
	cow_put_pages(0, pages - 1, 1);
	fxfs_sim_hold_commits(true);
	syncer = sim_spawn("syncer", cow_fsync, NULL);
	while (!fxfs_sim_commit_held())
		msleep(1);
	fxfs_sim_read(mount->info->fs, ino, 0, got, sizeof(got));
	check(!memcmp(got, old, sizeof(got)), "a crash before the commit doesn't show the file as it was");
	fxfs_sim_read_pending(mount->info->fs, ino, 0, got, sizeof(got));
	check(!memcmp(got, cow_expected, sizeof(got)),
	      "a crash after the commit wouldn't show what writeback copied");
	storer = sim_spawn("storer", cow_store_during_commit, NULL);
	msleep(50);
	fxfs_sim_hold_commits(false);
	wait_for(syncer);
	wait_for(storer);
	fxfs_sim_read(mount->info->fs, ino, 0, got, sizeof(got));
	check(!memcmp(got, cow_expected, sizeof(got)),
	      "after the held fsync, Fxfs doesn't hold the store made during it: page 3 holds round %llu",
	      *(u64 *)(got + 3 * PAGE_SIZE));
	cow_check(3, false, "after the held fsync");
	cow_check_file("after the next fsync", pages * PAGE_SIZE);
}

static void cow_write_page3(void *arg)
{
	cow_put(3, (uintptr_t)arg, false);
}

/*
 * fxfs.dax_rebind_now: the budget's worker writes back, seals and rebinds all it can, waited for as
 * a guest check waits: until the rounds it served reach the asks, as read after this one.
 */
static void cow_rebind_now(void *arg)
{
	u64 asked;

	check(!sim_param_set("dax_rebind_now", "1"), "rebinding now");
	asked = sim_dax_stat(mount, "dax_rebinds_asked");
	while (sim_dax_stat(mount, "dax_rebinds_done") < asked)
		msleep(1);
}

/* A file of `pages` pages, written back, sealed and mapped, each page private and clean. */
static void cow_mapped_file(u64 pages)
{
	cow_target = cow_file(pages);
	cow_addr = sim_mmap(cow_target, 0, pages * PAGE_SIZE);
}

/*
 * Runs `run` in a task of its own, which waits at `step` of cow_target's (dax_pause), as do any
 * others reaching it until dax_pause is cleared; once one waits there, the task.
 */
static struct task_struct *cow_pause(unsigned long step, void (*run)(void *), void *arg)
{
	struct task_struct *task;
	char value[24];

	snprintf(value, sizeof(value), "%lu", file_inode(cow_target)->i_ino);
	sim_param_set("dax_pause_ino", value);
	snprintf(value, sizeof(value), "%lu", step);
	sim_param_set("dax_pause", value);
	task = sim_spawn("paused", run, arg);
	while (!(sim_param_read("dax_paused") & step))
		msleep(1);
	return task;
}

/*
 * That writeback's swap meets no write(2) between the iomap that dirtied its page and its copy: the
 * write paused there (dax_pause), holding the file's semaphore shared, an fsync's swap waits for
 * it. Under the semaphore shared, the swap took the page, writeback copied the old bytes, and the
 * write's, copied after, were in neither set: the next fsync wrote nothing of them.
 */
static void cow_swap_during_write(void)
{
	struct task_struct *writer, *syncer;

	cow_mapped_file(8);
	/* A fault's page, which tags the mapping dirty: fsync's writepages runs. */
	cow_put(0, 1, true);
	writer = cow_pause(PAUSE_WRITE_MAPPED, cow_write_page3, (void *)1UL);
	syncer = sim_spawn("syncer", cow_fsync, NULL);
	msleep(50);
	sim_param_set("dax_pause", "0");
	wait_for(writer);
	wait_for(syncer);
	cow_check_file("the write a swap met", 8 * PAGE_SIZE);
}

/*
 * That a rebind waits for a writeback under way, from its swap to its commit (the file's
 * `wb_lock`): the commit held, a rebind found the pages clean, their extents Fxfs's old ones,
 * sealed, and mapped those over the private pages holding what the writeback was writing.
 */
static void cow_rebind_during_writeback(void)
{
	struct task_struct *syncer, *rebinder;
	u64 page;

	cow_mapped_file(8);
	cow_put_pages(0, 7, 1);
	fxfs_sim_hold_commits(true);
	syncer = sim_spawn("syncer", cow_fsync, NULL);
	while (!fxfs_sim_commit_held())
		msleep(1);
	rebinder = sim_spawn("rebinder", cow_rebind_now, NULL);
	msleep(50);
	fxfs_sim_hold_commits(false);
	wait_for(syncer);
	wait_for(rebinder);
	for (page = 0; page < 8; page++)
		cow_check(page, page % 2, "after a rebind met a writeback");
	cow_check_file("after a rebind met a writeback", 8 * PAGE_SIZE);
}

/*
 * That a rebind takes no dirty page, nor one a fault left writable: the worker's own writeback
 * held after its copy, a write(2) dirties page 1 again, which the rebind after must leave, while
 * it rebinds the rest, whose PTEs, made writable by faults, the writeback write-protected
 * (fxfs_dax_protect): through one left writable, a store makes the page
 * private again on the host, which the guest never learns of, and no fsync writes.
 */
static void cow_rebind_skips_dirty(void)
{
	struct task_struct *rebinder;
	u64 freed;

	cow_mapped_file(8);
	cow_put_pages(0, 7, 1);
	freed = sim_device_private_freed();
	fxfs_sim_hold_commits(true);
	rebinder = sim_spawn("rebinder", cow_rebind_now, NULL);
	while (!fxfs_sim_commit_held())
		msleep(1);
	cow_put(1, 2, false);
	fxfs_sim_hold_commits(false);
	wait_for(rebinder);
	check(sim_device_private_freed() > freed, "the worker rebound nothing");
	cow_check(1, false, "the page dirtied as the worker wrote back");
	cow_check(3, true, "a page a fault had made writable");
	/* Write-protected, it faults, and is dirty: a writable PTE left would store unseen. */
	cow_put(3, 3, true);
	cow_check_file("after the rebind", 8 * PAGE_SIZE);
}

static void cow_store_page3(void *arg)
{
	cow_put(3, (uintptr_t)arg, true);
}

static void cow_load_page3_checked(void *arg)
{
	cow_check(3, true, "a load's fault as a rebind's MAP was on its way");
}

/*
 * That a rebind holds the file's semaphore exclusively from its look at the dirty sets to its MAP:
 * the MAP held on its way to the host, a write(2) to a page it found clean waits, and so does a
 * fault, a store's or a load's, which blocks on the semaphore holding its DAX entry. Under the
 * semaphore shared, the write or the store dirtied and stored to the private page, which the MAP
 * then replaced with the extent's. Every other file's private pages are rebound first, so that the
 * MAP held is this file's.
 */
static void cow_rebind_against_write(void)
{
	static void (*const ways[])(void *) = {
		cow_write_page3, cow_store_page3, cow_load_page3_checked,
	};
	struct task_struct *rebinder, *other;
	size_t way;

	for (way = 0; way < ARRAY_SIZE(ways); way++) {
		wait_for(sim_spawn("rebinder", cow_rebind_now, NULL));
		cow_mapped_file(8);
		sim_device_hold_map();
		rebinder = sim_spawn("rebinder", cow_rebind_now, NULL);
		while (!sim_device_held_map())
			msleep(1);
		other = sim_spawn("other", ways[way], (void *)1UL);
		msleep(50);
		sim_device_release_map();
		wait_for(rebinder);
		wait_for(other);
		cow_check(3, way == 1, "after a rebind's MAP was on its way");
		cow_check_file("after the rebind", 8 * PAGE_SIZE);
	}
}

/*
 * That a writeback whose commit fails after its bytes (fxfs.dax_fail_commits) says so: the fsync
 * fails EIO, and Fxfs holds the file as it was, as a crash between the two would; the pages clean
 * then, as the page cache's are after a failed writeback, the next fsync succeeds, and what is
 * written again is kept.
 */
static void cow_commit_failed(void)
{
	static char old[8 * PAGE_SIZE], got[8 * PAGE_SIZE];
	int err;

	cow_mapped_file(8);
	memcpy(old, cow_expected, sizeof(old));
	cow_put_pages(0, 7, 1);
	sim_param_set("dax_fail_commits", "1");
	err = sim_fsync(cow_target);
	check(err == -EIO, "an fsync whose writeback's commit failed: %d", err);
	fxfs_sim_read(mount->info->fs, file_inode(cow_target)->i_ino, 0, got, sizeof(got));
	check(!memcmp(got, old, sizeof(got)), "a commit that failed changed the file in Fxfs");
	check(!sim_fsync(cow_target), "the fsync after the failed one");
	cow_put_pages(0, 7, 2);
	cow_check_file("written again", 8 * PAGE_SIZE);
}

/* Fills the window's ranges' half with chunks of new files, read: reclaim takes the oldest ranges. */
static void cow_fill_window(void)
{
	const size_t fillers = WINDOW_LEN / 2 / CHUNK_LEN + 2;
	size_t i;
	char byte;

	for (i = 0; i < fillers; i++) {
		struct file *filler = sim_file_new(mount);

		check(!sim_truncate(filler, CHUNK_LEN), "filler %zu's truncate", i);
		byte = 1;
		check(sim_read(filler, CHUNK_LEN - 1, &byte, 1) == 1 && !byte,
		      "filler %zu's read: %#x", i, byte);
	}
}

/*
 * That reclaim frees no range a writeback reads from (GCE r544's class: a range's window space
 * freed and given to another range while something still reads it there). A writeback paused
 * after its swap (dax_pause), before it reads its pages from the window, the window's ranges'
 * half is filled with whole chunks of other files, read, more than it holds: reclaim takes the
 * least recently used ranges, the writeback's first, unless the writeback holds it. Taken, the
 * next chunk's range maps zeros there, which the writeback hands Fxfs for the file's pages.
 */
static void cow_reclaim_during_writeback(void)
{
	struct task_struct *syncer;
	size_t i;

	cow_target = cow_file(8);
	/* By write(2) alone: a fault's dirty DAX entry would keep reclaim off the range anyway. */
	for (i = 0; i < 8; i++)
		cow_put(i, 1, false);
	syncer = cow_pause(PAUSE_WRITEBACK, cow_fsync, NULL);
	cow_fill_window();
	sim_param_set("dax_pause", "0");
	wait_for(syncer);
	cow_check_file("after reclaim met a writeback", 8 * PAGE_SIZE);
}

/*
 * That a writeback leaves no PTE writable of a page it took. A store's fault paused past its
 * iomap_end (dax_pause), its PTE writable, the file's semaphore let go and its page's DAX entry
 * held, the worker writes the page back, seals and rebinds it. dax_writeback_mapping_range, finding
 * the entry locked, gives up on it: with the write-protect its alone, the PTE stayed writable, and
 * the rebind's read-only MAP under it was refused, as the host would refuse the store; or, with no
 * rebind, a store through it after the writeback's copy went in no writeback (interleave's seed 22,
 * 15 of CI's 3,000).
 */
static void cow_writeback_during_fault(void)
{
	struct task_struct *storer, *rebinder;

	cow_mapped_file(8);
	storer = cow_pause(PAUSE_FAULTED, cow_store_page3, (void *)1UL);
	rebinder = sim_spawn("rebinder", cow_rebind_now, NULL);
	wait_for(rebinder);
	sim_param_set("dax_pause", "0");
	wait_for(storer);
	cow_check(3, true, "the store whose fault a writeback met");
	cow_put(3, 2, true);
	cow_check_file("after a writeback met a fault", 8 * PAGE_SIZE);
}

/*
 * An fsync of cow_target, or, if `arg`, a sync of the filesystem, then a look at Fxfs: each page's
 * round from before it began is there.
 */
static void cow_sync_kept(void *arg)
{
	static char got[8 * PAGE_SIZE];
	const char *what = arg ? "a sync" : "an fsync";

	if (arg)
		fxfs_vfs_make_room(file_inode(cow_target)->i_sb);
	else
		check(!sim_fsync(cow_target), "the second fsync");
	fxfs_sim_read(mount->info->fs, file_inode(cow_target)->i_ino, 0, got, sizeof(got));
	check(!memcmp(got, cow_expected, sizeof(got)),
	      "%s returned, another's writeback under way, and Fxfs holds round %llu of page 0, not 1",
	      what, *(u64 *)got);
}

/*
 * That an fsync, and a sync, wait for a writeback under way. One paused after its swap
 * (dax_pause), before it reads its pages from the window, the file has nothing dirty: an fsync
 * returned at once, and a sync's settle passed the file by, the stores before them neither written
 * nor sealed (interleave's seed 962, the budget's worker's writeback there).
 */
static void cow_fsync_during_writeback(void)
{
	struct task_struct *first, *second;
	uintptr_t sync;

	for (sync = 0; sync < 2; sync++) {
		cow_mapped_file(8);
		cow_put_pages(0, 7, 1);
		first = cow_pause(PAUSE_WRITEBACK, cow_fsync, NULL);
		second = sim_spawn("second", cow_sync_kept, (void *)sync);
		msleep(50);
		sim_param_set("dax_pause", "0");
		wait_for(first);
		wait_for(second);
		cow_check_file(sync ? "after a sync met a writeback" : "after an fsync met a writeback",
			       8 * PAGE_SIZE);
	}
}

static void cow_load_page3(void *arg)
{
	u64 got;

	check(!sim_user_load((uintptr_t)arg + 3 * PAGE_SIZE, &got, sizeof(got)), "a load of page 3");
}

/*
 * That reclaim takes a range whose pages a writeback cleaned. A store through one mapping tags page
 * 3's DAX entry dirty; a load through a second faults, paused past its iomap (dax_pause), holding
 * the entry, as an fsync writes the page back: dax_writeback_mapping_range gives up on the entry,
 * locked, which stays tagged, and with no page dirty no writeback comes for it again. Its
 * invalidation failing for ever, reclaim passed the range by each time, as other files' chunks
 * filled the window, and a fault short of window reclaimed it again and again (interleave's seed
 * 3390 at 1,000 operations, a PMD's entry, which loads of any of its pages hold). Taken, the read
 * after maps the chunk again.
 */
static void cow_reclaim_after_writeback_met_a_fault(void)
{
	struct task_struct *loader;
	unsigned long second;
	u64 maps;

	cow_mapped_file(8);
	cow_put(3, 1, true);
	second = sim_mmap(cow_target, 0, 8 * PAGE_SIZE);
	loader = cow_pause(PAUSE_READ_MAPPED, cow_load_page3, (void *)second);
	check(!sim_fsync(cow_target), "the fsync a load's fault met");
	sim_param_set("dax_pause", "0");
	wait_for(loader);
	sim_munmap(second);
	sim_munmap(cow_addr);
	cow_fill_window();
	maps = sim_device_maps();
	cow_check(3, false, "after the window filled");
	check(sim_device_maps() > maps, "reclaim left the range a writeback cleaned, its DAX entry tagged dirty");
	cow_check_file("after the window filled", 8 * PAGE_SIZE);
}

/*
 * That a page a writeback takes is the VM's own before its old block dies. The file's pages rebound
 * (dax_rebind_now), the window maps its sealed extent; a store's fault to page 3 paused past its
 * iomap_end (dax_pause): the page dirty, its store not come, the window mapping its block still. An
 * fsync writes the file back, every block moving, and the seals after have Fxfs free the old extent
 * (FreeAfter::Flush), where the window reads zeros: the store, let go, copied them as the host made
 * its page private, and the next writeback wrote them over the rest of page 3 (interleave's seed
 * 2802 at 1,000 operations).
 */
static void cow_writeback_before_store(void)
{
	struct task_struct *storer;
	u64 page, freed;
	int seals;

	cow_mapped_file(8);
	freed = sim_device_private_freed();
	wait_for(sim_spawn("rebinder", cow_rebind_now, NULL));
	check(sim_device_private_freed() >= freed + 8, "the file's pages weren't rebound");
	for (page = 0; page < 8; page++) {
		if (page != 3)
			cow_put(page, 1, page % 2);
	}
	storer = cow_pause(PAUSE_FAULTED, cow_store_page3, (void *)1UL);
	for (seals = 0; seals < 8; seals++)
		check(!sim_fsync(cow_target), "fsync %d, a store's fault paused", seals);
	sim_param_set("dax_pause", "0");
	wait_for(storer);
	cow_check_file("after a writeback took a page before its store", 8 * PAGE_SIZE);
}

static void cow_read_page0(void *arg)
{
	cow_check(0, false, "a read as a seal was under way");
}

static void cow_write_page1(void *arg)
{
	cow_put(1, 2, false);
}

static void cow_load_page0(void *arg)
{
	cow_check(0, true, "a load's fault as a seal was under way");
}

static void cow_store_page1(void *arg)
{
	cow_put(1, 2, true);
}

/*
 * That a range maps no extent a seal in progress has yet to seal. An fsync's writeback writes the
 * file to a new extent, and its seal is held once it has begun (fxfs_sim_hold_seals): Fxfs takes the
 * extent for sealed already, as the guest's does (OpenExtents::seal), the device not. The file's
 * ranges dropped, a read maps the chunk again, as a slice of the extent's region, and then a write,
 * as a range of its own; and a load's fault, and a store's, which block on the seal holding their
 * DAX entry: each from the extent, at once, which the host refused (-EPERM); doc-13's invariant 1,
 * the cow_ scenarios' tripwire, which mutants' schedules of cow_punch_dirty met.
 */
static void cow_map_during_seal(void)
{
	static void (*const ways[])(void *) = {
		cow_read_page0, cow_write_page1, cow_load_page0, cow_store_page1,
	};
	struct task_struct *syncer, *mapper;
	size_t way;

	for (way = 0; way < ARRAY_SIZE(ways); way++) {
		cow_mapped_file(8);
		cow_put_pages(0, 7, 1);
		fxfs_sim_hold_seals(true);
		syncer = sim_spawn("syncer", cow_fsync, NULL);
		while (!fxfs_sim_seal_held())
			msleep(1);
		drop_ranges(cow_target);
		mapper = sim_spawn("mapper", ways[way], NULL);
		msleep(50);
		fxfs_sim_hold_seals(false);
		wait_for(syncer);
		wait_for(mapper);
		cow_check_file("after a map met a seal", 8 * PAGE_SIZE);
	}
}

static void cow_truncate_to_a_page(void *arg)
{
	check(!sim_truncate(arg, PAGE_SIZE), "the truncate to a page");
}

static void cow_second_file(void *arg)
{
	cow_mapped_file((uintptr_t)arg);
}

/*
 * That the budget counts a private page until the host has freed it. A truncate's MAP of zeros over
 * 48 dirty private pages held on its way (sim_device_hold_map), a second file's write takes the
 * room among the 64 private_max allows: counted free as the MAP began, as an UNMAP's were, the 48
 * and the write's were on the host at once, which refused a store past private_max (interleave's
 * seed 90, a range's UNMAP). Counted till it ends, the write waits for the budget.
 */
static void cow_truncate_held(void)
{
	const u64 pages = 56;
	static char first_page[PAGE_SIZE];
	struct task_struct *truncater, *second;
	struct file *first;

	cow_mapped_file(pages);
	memcpy(first_page, cow_expected, PAGE_SIZE);
	cow_put_pages(8, pages - 1, 1);
	first = cow_target;
	sim_device_hold_map();
	truncater = sim_spawn("truncater", cow_truncate_to_a_page, first);
	while (!sim_device_held_map())
		msleep(1);
	second = sim_spawn("second", cow_second_file, (void *)24UL);
	msleep(50);
	sim_device_release_map();
	wait_for(truncater);
	wait_for(second);
	check(sim_device_private_most() <= COW_PRIVATE_MAX, "%llu private pages",
	      sim_device_private_most());
	check_file("the file truncated to a page", first, first_page, PAGE_SIZE);
	cow_check_file("the second file", 24 * PAGE_SIZE);
}

/*
 * That the iomap that dirties a page has the host make it private first, and fails the write if
 * the host doesn't (TASK-164.9): the file's pages rebound, mapping its sealed extent, a write(2) to
 * page 2 and a store to page 5, each with the device failing its PRIVATE, fail (-EIO, the fault's
 * SIGBUS), dirtying nothing, reserving nothing; tried again, they succeed. A page dirtied without
 * its PRIVATE is the window's mapping of an old block, which a writeback may move and Fxfs free
 * before its store comes (cow_writeback_before_store): the host, which this one makes refuse a
 * store to a page no PRIVATE made private, as the oracle of the rule, would have copied zeros.
 */
static void cow_private_failed(void)
{
	static const u64 pages[] = { 2, 5 };
	u64 freed, reserved, round = 1;
	size_t i;
	int err;

	cow_mapped_file(8);
	freed = sim_device_private_freed();
	wait_for(sim_spawn("rebinder", cow_rebind_now, NULL));
	check(sim_device_private_freed() >= freed + 8, "the file's pages weren't rebound");
	for (i = 0; i < ARRAY_SIZE(pages); i++) {
		loff_t at = pages[i] * PAGE_SIZE;
		bool mapped = i;

		reserved = fxfs_sim_reserved(mount->info->fs, file_inode(cow_target)->i_ino);
		sim_device_fail(VIRTIO_EXTENT_T_PRIVATE, 0, 1);
		err = mapped ? sim_user_store(cow_addr + at, &round, sizeof(round)) :
			       (int)sim_write(cow_target, at, &round, sizeof(round));
		check(err == (mapped ? -EFAULT : -EIO), "a %s whose PRIVATE failed: %d",
		      mapped ? "store" : "write", err);
		check(sim_device_failed() == i + 1, "no PRIVATE failed");
		check(fxfs_sim_reserved(mount->info->fs, file_inode(cow_target)->i_ino) == reserved,
		      "a %s whose PRIVATE failed left %llu bytes of room reserved, of %llu before",
		      mapped ? "store" : "write",
		      fxfs_sim_reserved(mount->info->fs, file_inode(cow_target)->i_ino), reserved);
		cow_check(pages[i], mapped, "after its PRIVATE failed");
		cow_put(pages[i], round, mapped);
	}
	cow_check_file("after PRIVATEs failed", 8 * PAGE_SIZE);
}

static const struct {
	const char *name;
	void (*run)(void);
	/* Whether the device holds few private pages (COW_PRIVATE_MAX), for the budget to throttle. */
	bool few;
} scenarios[] = {
#define SCENARIO(name) { #name, name, false }
#define COW_SCENARIO(name) { #name, name, true }
	SCENARIO(short_copy),
	SCENARIO(short_copy_pages),
	SCENARIO(same_page),
	SCENARIO(append_stopped_short_in_a_page),
	SCENARIO(store_past_end_then_written),
	SCENARIO(write_past_end_copies_nothing),
	SCENARIO(write_killed),
	SCENARIO(unmap_failed),
	SCENARIO(allocations_failed),
	COW_SCENARIO(cow_rebind_against_stores),
	COW_SCENARIO(cow_writeback_against_stores),
	COW_SCENARIO(cow_truncate_dirty),
	COW_SCENARIO(cow_punch_dirty),
	COW_SCENARIO(cow_crash_between_bytes_and_commit),
	COW_SCENARIO(cow_swap_during_write),
	COW_SCENARIO(cow_rebind_during_writeback),
	COW_SCENARIO(cow_rebind_skips_dirty),
	COW_SCENARIO(cow_rebind_against_write),
	COW_SCENARIO(cow_commit_failed),
	COW_SCENARIO(cow_reclaim_during_writeback),
	COW_SCENARIO(cow_writeback_during_fault),
	COW_SCENARIO(cow_fsync_during_writeback),
	COW_SCENARIO(cow_truncate_held),
	COW_SCENARIO(cow_reclaim_after_writeback_met_a_fault),
	COW_SCENARIO(cow_writeback_before_store),
	COW_SCENARIO(cow_map_during_seal),
	COW_SCENARIO(cow_private_failed),
};

static void scenario(void *arg)
{
	mount = sim_mount_new();
	((void (*)(void))arg)();
}

int main(int argc, char **argv)
{
	size_t i;

	sim_init(1);
	for (i = 0; i < ARRAY_SIZE(scenarios); i++) {
		if (argc < 2) {
			sim_log("%s\n", scenarios[i].name);
		} else if (!strcmp(argv[1], scenarios[i].name)) {
			sim_device_init(WINDOW_LEN);
			if (scenarios[i].few)
				sim_device_private_max(COW_PRIVATE_MAX);
			sim_spawn(scenarios[i].name, scenario, scenarios[i].run);
			sim_run();
			sim_log("%s: passed\n", scenarios[i].name);
			return 0;
		}
	}
	return argc < 2 ? 0 : 1;
}
