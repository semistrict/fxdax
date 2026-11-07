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
#define PAUSE_COUNTED BIT(10)
/* A chunk's range, the window's whole ranges, at a chunk's alignment. */
#define CHUNK_LEN (2UL << 20)
/*
 * The device's private pages, few, so that the budget, past seven eighths of them, has the worker
 * write back and rebind while tasks store.
 */
#define COW_PRIVATE_MAX 64

/* What it expects, void once a sweep's call has failed (kernel/sweep.c). */
#define check sim_expect

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

/*
 * That every way the model of Fxfs grows a file reads zeros where a cut that kept its last block
 * (FXFS_SET_KEEP_TAIL), as a DAX file's truncate does, left bytes past the end, as Fxfs does
 * (fxfs-posix's every_growth_finds_what_a_cut_left_past_the_end_zeroed): a writeback in that block
 * keeps of it what was the file's alone (read_unaligned_ends); one past it, a truncate, and a clone
 * past it, even of that block, zero the tail of a block they don't write (txn_grow_size,
 * zero_tail_before_growth). Where the model kept them, a writeback of a DAX file's last page up to
 * its end, and one past the end after it, read them back (failures 140272 at 1,000).
 */
static void fxfs_growths_zero_past_a_cut(void)
{
	static const struct {
		const char *what;
		u64 size;
	} growths[] = {
		{ "a write in the block", 2000 },
		{ "a write past it", 65537 },
		{ "a truncate", 65537 },
		{ "a clone past it", 65536 + 2 * PAGE_SIZE },
		{ "a clone of the block past it", 65536 + PAGE_SIZE },
	};
	struct fxfs_setattr cut = { .valid = FXFS_SET_SIZE | FXFS_SET_KEEP_TAIL, .size = 1000 };
	struct fxfs *fs = mount->info->fs;
	static char bytes[2 * PAGE_SIZE], expected[PAGE_SIZE], got[PAGE_SIZE];
	struct fxfs_write_part part = { 0, bytes, sizeof(bytes) };
	struct fxfs_setattr grown = { .valid = FXFS_SET_SIZE };
	u64 source = fxfs_sim_new_file(fs), ino;
	struct fxfs_attr attr;
	size_t i, at;

	memset(bytes, 's', sizeof(bytes));
	check(!fxfs_write_many(fs, source, &part, 1, 0), "the clones' source's write");
	memset(bytes, 'c', sizeof(bytes));
	for (i = 0; i < ARRAY_SIZE(growths); i++) {
		ino = fxfs_sim_new_file(fs);
		part = (struct fxfs_write_part) { 0, bytes, sizeof(bytes) };
		check(!fxfs_write_many(fs, ino, &part, 1, 0), "%s: the write", growths[i].what);
		check(!fxfs_write_inode(fs, ino, &cut), "%s: the cut", growths[i].what);
		switch (i) {
		case 0:
			part = (struct fxfs_write_part) { 1999, "w", 1 };
			check(!fxfs_write_many(fs, ino, &part, 1, 0), "%s", growths[i].what);
			break;
		case 1:
			part = (struct fxfs_write_part) { 65536, "w", 1 };
			check(!fxfs_write_many(fs, ino, &part, 1, 0), "%s", growths[i].what);
			break;
		case 2:
			grown.size = growths[i].size;
			check(!fxfs_write_inode(fs, ino, &grown), "%s", growths[i].what);
			break;
		case 3:
			check(!fxfs_clone_range(fs, source, 0, 2 * PAGE_SIZE, ino, 65536,
						growths[i].size, &attr), "%s", growths[i].what);
			break;
		case 4:
			check(!fxfs_clone_range(fs, ino, 0, PAGE_SIZE, ino, 65536, growths[i].size,
						&attr), "%s", growths[i].what);
			break;
		}
		check(fxfs_sim_size(fs, ino) == growths[i].size, "%s: Fxfs's size is %llu",
		      growths[i].what, fxfs_sim_size(fs, ino));
		memset(expected, 0, sizeof(expected));
		memset(expected, 'c', 1000);
		if (i == 0)
			expected[1999] = 'w';
		fxfs_sim_read(fs, ino, 0, got, sizeof(got));
		for (at = 0; at < sizeof(got); at++)
			check(got[at] == expected[at], "%s: byte %zu is %#x, not %#x", growths[i].what,
			      at, (u8)got[at], (u8)expected[at]);
		/* The clone of the block shares it as the growth left it. */
		if (i == 4) {
			fxfs_sim_read(fs, ino, 65536, got, sizeof(got));
			for (at = 0; at < sizeof(got); at++)
				check(got[at] == expected[at], "%s: byte %zu of the clone is %#x, not %#x",
				      growths[i].what, at, (u8)got[at], (u8)expected[at]);
		}
	}
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
	sim_await(sim_param_read("dax_paused") & PAUSE_WRITE_MAPPED, "no write waits mapped");
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

/*
 * That a write the window can never have room for fails, ENOSPC, rather than wait for ever: the
 * ranges' half of the window, chunk by chunk, kept from use as each UNMAP of it fails (TASK-44.22).
 * The budget's worker's rounds, which each wait for room asks for, counted as ranges let go of, and
 * the write waited for ever (read_pressure in guest/sim's sweep, a failed UNMAP taking its small
 * window's aligned room).
 */
static void cow_rebind_now(void *arg);

static void unmaps_failed_take_the_window(void)
{
	static char chunk[CHUNK_LEN];
	const int chunks = WINDOW_LEN / 2 / CHUNK_LEN;
	struct file *file;
	ssize_t done;
	int i;

	memset(chunk, 'w', sizeof(chunk));
	for (i = 0; i < chunks; i++) {
		/* Its pages private no more, which an UNMAP that failed keeps counted for good. */
		file = new_file(chunk, sizeof(chunk));
		check(!sim_fsync(file), "file %d's fsync", i);
		cow_rebind_now(NULL);
		sim_device_fail(VIRTIO_EXTENT_T_UNMAP, 0, 1);
		check(!sim_truncate(file, 0), "truncating file %d", i);
		check(sim_device_failed() == i + 1, "file %d's UNMAP didn't fail", i);
	}
	done = sim_write(sim_file_new(mount), 0, chunk, sizeof(chunk));
	check(done == -ENOSPC, "a write with no room in the window for its chunk: %zd", done);
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
	sim_await(fxfs_sim_commit_held(), "no writeback's commit held");
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
	sim_await(sim_dax_stat(mount, "dax_rebinds_done") >= asked, "no rebind came");
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
	sim_await(sim_param_read("dax_paused") & step, "no task waits at step %#lx", step);
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
	sim_await(fxfs_sim_commit_held(), "no writeback's commit held");
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
	sim_await(fxfs_sim_commit_held(), "no writeback's commit held");
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
		sim_device_hold(VIRTIO_EXTENT_T_MAP);
		rebinder = sim_spawn("rebinder", cow_rebind_now, NULL);
		sim_await(sim_device_held(), "no MAP held");
		other = sim_spawn("other", ways[way], (void *)1UL);
		msleep(50);
		sim_device_release();
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
		sim_await(fxfs_sim_seal_held(), "no seal held");
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
 * 48 dirty private pages held on its way (sim_device_hold), a second file's write takes the
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
	sim_device_hold(VIRTIO_EXTENT_T_MAP);
	truncater = sim_spawn("truncater", cow_truncate_to_a_page, first);
	sim_await(sim_device_held(), "no MAP held");
	second = sim_spawn("second", cow_second_file, (void *)24UL);
	msleep(50);
	sim_device_release();
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
 * SIGBUS), dirtying nothing, reserving nothing; tried again, they succeed; and a write to page 3
 * whose PRIVATE the guest found no memory for fails ENOMEM, one the host had no room for EIO. A page dirtied without
 * its PRIVATE is the window's mapping of an old block, which a writeback may move and Fxfs free
 * before its store comes (cow_writeback_before_store): the host, which this one makes refuse a
 * store to a page no PRIVATE made private, as the oracle of the rule, would have copied zeros.
 */
static void cow_private_failed(void)
{
	/*
	 * The store's page before the write's: the write, tried again where it failed, follows on
	 * from itself, and makes the pages after it private ahead (TASK-164.10).
	 */
	static const u64 pages[] = { 5, 2 };
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
	/*
	 * fxdax: a PRIVATE the guest had no memory to ask for fails the write ENOMEM, as an
	 * allocation's failure does, not EIO, which says data is lost (GCE r698, TASK-44.15); one the
	 * host refused for want of room, past private_max, the guest's count and the host's apart,
	 * EIO (TASK-170). Neither dirties the page nor keeps room for it.
	 */
	for (i = 0; i < 2; i++) {
		bool guest = !i;

		reserved = fxfs_sim_reserved(mount->info->fs, file_inode(cow_target)->i_ino);
		if (guest)
			sim_alloc_fail(0, 1);
		else
			sim_device_fail_with(VIRTIO_EXTENT_T_PRIVATE, 0, 1, -ENOMEM);
		err = (int)sim_write(cow_target, 3 * PAGE_SIZE, &round, sizeof(round));
		check(err == (guest ? -ENOMEM : -EIO), "a write whose PRIVATE %s: %d",
		      guest ? "the guest had no memory for" : "the host had no room for", err);
		check(fxfs_sim_reserved(mount->info->fs, file_inode(cow_target)->i_ino) == reserved,
		      "a write whose PRIVATE found no memory left room reserved");
		cow_check(3, false, "after its PRIVATE found no memory");
	}
	cow_put(3, round, false);
	cow_check_file("after PRIVATEs failed", 8 * PAGE_SIZE);
}

/*
 * fxdax: that a writeback Fxfs finds no memory for (TASK-207) fails its fsync ENOMEM, Fxfs holding
 * the file as it was, and keeps its pages dirty, their room reserved again: a rebind, which maps
 * the file's sealed extents over its clean private pages, leaves them as written, as does a clone,
 * which takes the store's blocks, and the next fsync writes them. Clean, as an I/O error's, the
 * clone carried the blocks they replaced, and the rebind brought those back
 * (fxfs-guest-fsx-faults-dax, GCE r699).
 */
static void cow_writeback_found_no_memory(void)
{
	static char old[8 * PAGE_SIZE], got[8 * PAGE_SIZE];
	struct fxfs *fs = mount->info->fs;
	struct file *copy;
	u64 page;
	int err;

	cow_mapped_file(8);
	memcpy(old, cow_expected, sizeof(old));
	cow_put_pages(0, 7, 1);
	fxfs_sim_fail_commits_with(1, -ENOMEM);
	err = sim_fsync(cow_target);
	check(err == -ENOMEM, "an fsync whose writeback found no memory: %d", err);
	fxfs_sim_read(fs, file_inode(cow_target)->i_ino, 0, got, sizeof(got));
	check(!memcmp(got, old, sizeof(got)), "a writeback that found no memory changed the file in Fxfs");
	check(fxfs_sim_reserved(fs, file_inode(cow_target)->i_ino) == 8 * PAGE_SIZE,
	      "a writeback that found no memory left %llu bytes of room reserved for its 8 pages",
	      fxfs_sim_reserved(fs, file_inode(cow_target)->i_ino));
	/* A clone takes the store's blocks, once the VFS has written the source back. */
	copy = new_file(old, sizeof(old));
	check(!sim_clone(cow_target, 0, copy, 0, sizeof(got)), "the clone");
	check_file("the clone of a file whose writeback found no memory", copy, cow_expected,
		   sizeof(got));
	wait_for(sim_spawn("rebinder", cow_rebind_now, NULL));
	for (page = 0; page < 8; page++)
		cow_check(page, page % 2, "after a writeback that found no memory, and a rebind");
	cow_check_file("written back after one found no memory", 8 * PAGE_SIZE);
	fxfs_sim_read(fs, file_inode(cow_target)->i_ino, 0, got, sizeof(got));
	check(!memcmp(got, cow_expected, sizeof(got)), "Fxfs doesn't hold what was written");
}

static bool cow_pinned_stored;

static void cow_store_page0(void *arg)
{
	cow_put(0, 1, true);
	WRITE_ONCE(cow_pinned_stored, true);
}

/*
 * That a store to a page of a slice a pin holds waits for the pin to go (TASK-79.49, TASK-149.31):
 * the file's ranges dropped, a load maps its chunk again as a slice of its extent's region, and a
 * pin of page 0, as vmsplice takes one (sim_pin_user_page), holds the page. A store to it can't use
 * the slice (-EXDEV), whose drop waits for the page to be unused, as a truncate does: a fault from
 * user mode holds no pins. One that failed rather than wait, as a fault from the kernel does, which
 * may hold the pin itself (get_user_pages), was a SIGBUS (-EFAULT here) where the pin goes soon.
 * The guest check slice_held's steps, which went with writable DAX.
 */
static void cow_store_to_pinned_slice(void)
{
	struct task_struct *storer;
	struct page *pin;

	cow_mapped_file(8);
	drop_ranges(cow_target);
	cow_check(0, true, "the slice's load");
	pin = sim_pin_user_page(cow_addr);
	check(pin, "pinning page 0");
	storer = sim_spawn("storer", cow_store_page0, NULL);
	msleep(50);
	check(!READ_ONCE(cow_pinned_stored),
	      "the store to page 0 didn't wait for its pin: page 0 wasn't a slice's");
	if (pin)
		sim_unpin_page(pin);
	wait_for(storer);
	cow_check(0, true, "after the pin went");
	cow_check_file("after a store to a pinned slice's page", 8 * PAGE_SIZE);
}

static struct file *counted_file;

static void counted_fsync(void *arg)
{
	check(!sim_fsync(counted_file), "the fsync a store met");
}

/*
 * That a writeback counts its file's dirty ranges again once it holds the semaphore for writing.
 * Paused between its count and its swap (dax_pause), a store to the file's first chunk dirties a
 * range it didn't count: with a batch as large as the count, the swap took that range, first in the
 * file, and passed the second's by, written before the fsync began, which returned 0 without it
 * (the file model's, failures seed 62).
 */
static void cow_writeback_counts_again(void)
{
	static char bytes[BIG_LEN], got[6];
	const loff_t second = CHUNK_LEN + 5 * PAGE_SIZE;
	struct task_struct *syncer;
	unsigned long addr;
	char value[24];

	fill(bytes, sizeof(bytes), 3);
	counted_file = new_file(bytes, sizeof(bytes));
	check(!sim_fsync(counted_file), "the file's fsync");
	addr = sim_mmap(counted_file, 0, sizeof(bytes));
	check(sim_write(counted_file, second, "second", 6) == 6, "the second chunk's write");
	snprintf(value, sizeof(value), "%lu", file_inode(counted_file)->i_ino);
	sim_param_set("dax_pause_ino", value);
	snprintf(value, sizeof(value), "%lu", PAUSE_COUNTED);
	sim_param_set("dax_pause", value);
	syncer = sim_spawn("syncer", counted_fsync, NULL);
	sim_await(sim_param_read("dax_paused") & PAUSE_COUNTED, "no writeback waits, counted");
	check(!sim_user_store(addr + 3 * PAGE_SIZE, "first", 5), "the first chunk's store");
	sim_param_set("dax_pause", "0");
	wait_for(syncer);
	fxfs_sim_read(mount->info->fs, file_inode(counted_file)->i_ino, second, got, sizeof(got));
	check(!memcmp(got, "second", sizeof(got)),
	      "the fsync returned without the second chunk's write, before it began");
}

/* Writes `page` of cow_target whole by write(2), its round `round`, the rest as cow_file has it. */
static void cow_write_whole(u64 page, u64 round)
{
	loff_t at = page * PAGE_SIZE;

	memcpy(cow_expected + at, &round, sizeof(round));
	check(sim_write(cow_target, at, cow_expected + at, PAGE_SIZE) == PAGE_SIZE,
	      "a write of page %llu", page);
	WRITE_ONCE(cow_round[page], round);
}

/*
 * TASK-164.10: that write(2)s following on from each other, as a log's are, make pages private
 * ahead of them, more as more follow, in the PRIVATE of the write that needs one, and that those
 * after take none: 128 pages written in order, none private as they begin, take four PRIVATEs (the
 * first write, then 16, 64 and the 44 left ahead), where each took its own. A write elsewhere,
 * following on from nothing, makes none ahead. What the pages made private ahead held, they hold,
 * and the file is as written. Red with nothing made private ahead: 128 PRIVATEs.
 */
/*
 * That a write(2) from a file's own mapping, copying on write, its source a page on from its
 * target, apart from it in a page they share, then stores through the mapping over both, leave
 * the file as written and stored, each store to a page made private (TASK-149.16's hostile
 * scripts' writes from a file's mapping, as same_page has them without copying on write).
 */
static void cow_write_from_its_own_mapping(void)
{
	static char bytes[13604];
	const size_t src = 5674, len = 5136, at = 6684;
	ssize_t done;

	cow_mapped_file(8);
	wait_for(sim_spawn("rebinder", cow_rebind_now, NULL));
	done = sim_write_user(cow_target, 0, cow_addr + src, len);
	check(done == len, "a write from its own mapping: %zd", done);
	memmove(cow_expected, cow_expected + src, len);
	fill(bytes, sizeof(bytes), 9);
	check(!sim_user_store(cow_addr + at, bytes, sizeof(bytes)), "the stores after");
	memcpy(cow_expected + at, bytes, sizeof(bytes));
	cow_check_file("after the stores", 8 * PAGE_SIZE);
}

/*
 * As cow_write_from_its_own_mapping, of a file of holes read in again, after a write from memory
 * that stops short of its end, and with stores far on between (TASK-149.16, GCE r861's script but
 * for its fork, which fxfs-guest-checks' cow_fork_after_a_split_pmd has).
 */
static void cow_write_from_its_own_mapping_of_holes(void)
{
	static char source[PAGE_SIZE], stores[13604], far[10236];
	const size_t size = 256 * PAGE_SIZE - 37;
	struct file *file = sim_file_new(mount);
	char *expected = kzalloc(size, GFP_KERNEL);
	ssize_t done;

	check(!sim_truncate(file, size), "the truncate");
	check(!sim_fsync(file), "the fsync");
	sim_file_evict(file);
	cow_target = sim_file_read_in(mount, file);
	cow_addr = sim_mmap(cow_target, 0, size);
	fill(source, sizeof(source), 3);
	done = sim_write_user(cow_target, size - 7264, stops_after(source, 580), 2556);
	check(done == 580, "a write from memory ending part way: %zd", done);
	memcpy(expected + size - 7264, source + sizeof(source) - 580, 580);
	done = sim_write_user(cow_target, 0, cow_addr + 5674, 5136);
	check(done == 5136, "a write from its own mapping: %zd", done);
	memmove(expected, expected + 5674, 5136);
	fill(far, sizeof(far), 7);
	check(!sim_user_store(cow_addr + 676639, far, sizeof(far)), "the stores far on");
	memcpy(expected + 676639, far, sizeof(far));
	check_file("after the stores far on", cow_target, expected, size);
	fill(stores, sizeof(stores), 9);
	check(!sim_user_store(cow_addr + 6684, stores, sizeof(stores)), "the stores over both");
	memcpy(expected + 6684, stores, sizeof(stores));
	check_file("after the stores over both", cow_target, expected, size);
	kfree(expected);
}

static void cow_writes_ahead(void)
{
	u64 privates, ahead, page;

	cow_mapped_file(COW_PAGES);
	wait_for(sim_spawn("rebinder", cow_rebind_now, NULL));
	check(!sim_device_private(), "%llu pages private before the writes", sim_device_private());
	privates = sim_dax_stat(mount, "dax_privates");
	ahead = sim_dax_stat(mount, "dax_private_ahead_pages");
	for (page = 0; page < COW_PAGES; page++)
		cow_write_whole(page, 1);
	check(sim_dax_stat(mount, "dax_privates") - privates == 4,
	      "%llu PRIVATEs for %d pages written in order",
	      sim_dax_stat(mount, "dax_privates") - privates, COW_PAGES);
	check(sim_dax_stat(mount, "dax_private_ahead_pages") - ahead == 16 + 64 + 44,
	      "%llu pages made private ahead", sim_dax_stat(mount, "dax_private_ahead_pages") - ahead);
	cow_check_file("after writes in order", COW_PAGES * PAGE_SIZE);
	wait_for(sim_spawn("rebinder", cow_rebind_now, NULL));
	privates = sim_dax_stat(mount, "dax_privates");
	ahead = sim_dax_stat(mount, "dax_private_ahead_pages");
	cow_write_whole(40, 2);
	cow_write_whole(7, 2);
	check(sim_dax_stat(mount, "dax_privates") - privates == 2, "%llu PRIVATEs for two writes apart",
	      sim_dax_stat(mount, "dax_privates") - privates);
	check(sim_dax_stat(mount, "dax_private_ahead_pages") == ahead,
	      "%llu pages made private ahead of writes apart",
	      sim_dax_stat(mount, "dax_private_ahead_pages") - ahead);
	check(sim_device_private() == 2, "%llu pages private", sim_device_private());
	cow_check_file("after writes apart", COW_PAGES * PAGE_SIZE);
}

/* Writes the page `arg` of cow_target whole, its round 2. */
static void cow_write_round_two(void *arg)
{
	cow_write_whole((uintptr_t)arg, 2);
}

/*
 * TASK-164.11: that a write(2) has the pages it writes made private before it takes the inode's
 * lock, which the file's other writes take: its PRIVATE held on its way to the host
 * (sim_device_hold), a write to another page of the file, one private already, ends meanwhile;
 * then the first, which took the one PRIVATE, made before its lock, its iomap asking none. A
 * forked pgbench's writes waited for others' PRIVATEs as long as the PRIVATEs took (GCE r806: 37
 * of their 63 s). Red with the PRIVATE made under the lock, by the write's iomap: the second write
 * waits for the first's PRIVATE.
 */
static void cow_writes_private_before_the_lock(void)
{
	struct task_struct *first, *second;
	u64 privates, before;

	cow_mapped_file(COW_PAGES);
	wait_for(sim_spawn("rebinder", cow_rebind_now, NULL));
	cow_write_whole(60, 1);
	privates = sim_dax_stat(mount, "dax_privates");
	before = sim_dax_stat(mount, "dax_private_before_pages");
	sim_device_hold(VIRTIO_EXTENT_T_PRIVATE);
	first = sim_spawn("first", cow_write_round_two, (void *)10UL);
	sim_await(sim_device_held(), "no PRIVATE held");
	second = sim_spawn("second", cow_write_round_two, (void *)60UL);
	sim_await(READ_ONCE(cow_round[60]) == 2,
		  "a write to a private page waited for another write's PRIVATE");
	sim_device_release();
	wait_for(first);
	wait_for(second);
	check(sim_dax_stat(mount, "dax_privates") - privates == 1, "%llu PRIVATEs for one page",
	      sim_dax_stat(mount, "dax_privates") - privates);
	check(sim_dax_stat(mount, "dax_private_before_pages") - before == 1,
	      "%llu pages made private before the lock",
	      sim_dax_stat(mount, "dax_private_before_pages") - before);
	cow_check_file("after the writes", COW_PAGES * PAGE_SIZE);
}

/*
 * TASK-164.10: that a rebind maps a range's private pages back in one request, however many runs
 * they are: every other page of 64 written, out of order (none made private ahead), and written
 * back, 32 runs of a page, are rebound by one MAP_MANY, where each run took a MAP of its own. Every
 * page private is freed, and the file holds what was written. Red with a MAP a run: 32 requests.
 */
static void cow_rebinds_map_ranges_at_once(void)
{
	u64 requests, freed, page;

	cow_mapped_file(64);
	wait_for(sim_spawn("rebinder", cow_rebind_now, NULL));
	check(!sim_device_private(), "%llu pages private before the writes", sim_device_private());
	/* The first half up, then the second down: none follows on, from the file's making either. */
	for (page = 0; page < 64; page += 2)
		cow_write_whole(page < 32 ? page : 94 - page, 1);
	check(sim_device_private() == 32, "%llu pages private", sim_device_private());
	check(!sim_fsync(cow_target), "the fsync");
	requests = sim_device_map_requests();
	freed = sim_device_private_freed();
	wait_for(sim_spawn("rebinder", cow_rebind_now, NULL));
	check(sim_device_map_requests() - requests == 1, "%llu requests rebound 32 runs of a range",
	      sim_device_map_requests() - requests);
	check(sim_device_private_freed() - freed == 32, "%llu private pages freed",
	      sim_device_private_freed() - freed);
	check(!sim_device_private(), "%llu pages private after the rebind", sim_device_private());
	cow_check_file("after the rebind", 64 * PAGE_SIZE);
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
	SCENARIO(fxfs_growths_zero_past_a_cut),
	SCENARIO(write_killed),
	SCENARIO(unmap_failed),
	SCENARIO(unmaps_failed_take_the_window),
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
	COW_SCENARIO(cow_writeback_found_no_memory),
	COW_SCENARIO(cow_reclaim_during_writeback),
	COW_SCENARIO(cow_writeback_during_fault),
	COW_SCENARIO(cow_fsync_during_writeback),
	COW_SCENARIO(cow_truncate_held),
	COW_SCENARIO(cow_reclaim_after_writeback_met_a_fault),
	COW_SCENARIO(cow_writeback_before_store),
	COW_SCENARIO(cow_map_during_seal),
	COW_SCENARIO(cow_private_failed),
	COW_SCENARIO(cow_store_to_pinned_slice),
	COW_SCENARIO(cow_writeback_counts_again),
	SCENARIO(cow_writes_ahead),
	SCENARIO(cow_write_from_its_own_mapping),
	SCENARIO(cow_write_from_its_own_mapping_of_holes),
	SCENARIO(cow_writes_private_before_the_lock),
	SCENARIO(cow_rebinds_map_ranges_at_once),
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
			sim_end(SIM_END_READ_FILES);
			sim_log("%s: passed\n", scenarios[i].name);
			return 0;
		}
	}
	return argc < 2 ? 0 : 1;
}
