// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * Paths no seed of tests/interleave takes, each played out exactly: memory that ends part way
 * through a write, a write from the pages it writes, what a clone does to an open extent,
 * allocations and device requests that fail, and a MAP the host is slow to take.
 *
 *   scenarios [NAME]
 *
 * runs the scenario NAME, or, with none, lists them: the process is one scenario's, as sim_init
 * takes one seed, and a sweep of allocations fails each in turn within it.
 */

#include <sim/sim.h>
#include <linux/virtio_extent.h>

#define WINDOW_LEN (64UL << 20)
/* A chunk's data and more, written so as to take two extents, which only a range of its own maps. */
#define BIG_LEN (3UL << 20)
#define BIG_FIRST (1UL << 20)
#define PAUSE_READ_MAPPED BIT(4)
#define PAUSE_WRITE_MAPPED BIT(5)

#define check(cond, ...) do { if (!(cond)) sim_panic(__VA_ARGS__); } while (0)

static struct sim_mount *mount;

static void fill(char *buf, size_t len, unsigned char seed)
{
	size_t i;

	for (i = 0; i < len; i++)
		buf[i] = seed + i % 251;
}

/*
 * That read(2) and Fxfs's extents hold `expected`, now and once what the writes before left dead is
 * freed, as a range mapping blocks where they were then reads zeros.
 */
static void check_file(const char *what, struct file *file, const char *expected, size_t len)
{
	char *got = kmalloc(len, GFP_KERNEL);
	struct inode *inode = file_inode(file);
	int round;
	ssize_t read;

	for (round = 0; round < 2; round++) {
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
	/* Each page from the one after it, which the page before's move mustn't take. */
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
 * That an append stopped short leaves the range where Fxfs has the file's last block: Fxfs moved
 * it as the file was cut to what was written, and the next append's bytes went where it was.
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

/*
 * That a clone's blocks in an open extent aren't mapped writably outside the gate, past what a
 * writable region maps: shared, they keep away from it, read-only, in a range's own space. Through
 * it, as they read as sealed and went, their MAP was writable, which a seal may cross, and
 * fxfs_device_assert_in_gate's lockdep says so.
 */
static void clone_past_a_writable_region(void)
{
	static char one[PAGE_SIZE], many[32 * PAGE_SIZE], got[32 * PAGE_SIZE];
	struct file *first, *cloned, *clone = sim_file_new(mount);
	ssize_t done;

	fill(one, sizeof(one), 1);
	fill(many, sizeof(many), 9);
	first = new_file(one, sizeof(one));
	cloned = new_file(many, sizeof(many));
	/* What grows the region in the background holds it: over, the region goes with the ranges. */
	msleep(10);
	check(!sim_clone(cloned, 0, clone, 0, ARRAY_SIZE(many)), "the clone");
	drop_ranges(first);
	done = sim_read(first, 0, got, sizeof(one));
	check(done == sizeof(one) && !memcmp(got, one, sizeof(one)), "the first file: %zd", done);
	done = sim_read(cloned, 0, got, sizeof(many));
	check(done == sizeof(many) && !memcmp(got, many, sizeof(many)), "the cloned file: %zd", done);
	done = sim_read(clone, 0, got, sizeof(many));
	check(done == sizeof(many) && !memcmp(got, many, sizeof(many)), "its clone: %zd", done);
}

/*
 * As clone_past_a_writable_region, the clone in two pieces, which are no slice: GCE r423's stack,
 * fxfs_dax_map_chunk outside the gate, a writable MAP past the region's end as a seal shrank it
 * (generic/013), by a way into regions that fafefbac7 took out.
 */
static void clone_pieces_past_a_writable_region(void)
{
	static char one[PAGE_SIZE], many[32 * PAGE_SIZE], got[2 * PAGE_SIZE];
	struct file *first, *cloned, *clone = sim_file_new(mount);
	ssize_t done;

	fill(one, sizeof(one), 1);
	fill(many, sizeof(many), 9);
	first = new_file(one, sizeof(one));
	cloned = new_file(many, sizeof(many));
	msleep(10);
	check(!sim_clone(cloned, 20 * PAGE_SIZE, clone, 20 * PAGE_SIZE, 2 * PAGE_SIZE), "the clone");
	check(!sim_clone(cloned, 24 * PAGE_SIZE, clone, 24 * PAGE_SIZE, 2 * PAGE_SIZE), "the second clone");
	drop_ranges(first);
	done = sim_read(first, 0, got, sizeof(one));
	check(done == sizeof(one) && !memcmp(got, one, sizeof(one)), "the first file: %zd", done);
	done = sim_read(clone, 24 * PAGE_SIZE, got, sizeof(got));
	check(done == sizeof(got) && !memcmp(got, many + 24 * PAGE_SIZE, sizeof(got)),
	      "its clone: %zd", done);
}

static struct file *slice_file;
static char slice_bytes[2 * PAGE_SIZE], slice_written[2 * PAGE_SIZE];
static unsigned long slice_addr;

/* A fault, whose iomap holds the slice with no lock of the inode's that a write(2) waits for. */
static void load_slice(void *arg)
{
	char got[PAGE_SIZE];
	int err = sim_user_load(slice_addr, got, sizeof(got));

	/* It paused until the write was done, which had no entry of the fault's to wait for. */
	check(!err && !memcmp(got, slice_written, sizeof(got)), "the load: %d", err);
}

static void write_slice(void *arg)
{
	ssize_t done = sim_write(slice_file, 0, slice_written, sizeof(slice_written));

	check(done == sizeof(slice_written), "the write: %zd", done);
}

/*
 * That a write a slice refuses part way leaves no page writable a seal doesn't forget. A clone's file
 * had blocks in an open extent, and no writable page: so a slice of it, read-only, of a writable
 * region, on no list of files, took a write whose first page stayed where it was, the file cloned
 * having let go of it, and whose second moved. The first page's bit was set, the file unlisted,
 * and the slice stayed, a reader using it, past a seal (dax_check_seals' WARN). Shared blocks make
 * no slice now (TASK-153.3): the same steps, on a range of its own, whose writer doesn't wait for
 * the fault holding the page's entry, which then reads what it wrote.
 */
static void slice_refuses_a_piece(void)
{
	static char one[PAGE_SIZE], got[2 * PAGE_SIZE];
	struct file *holder, *cloned;
	char ino[24];
	ssize_t done;

	sim_param_set("dax_check_seals", "1");
	fill(one, sizeof(one), 1);
	fill(slice_bytes, sizeof(slice_bytes), 3);
	memset(slice_written, 'w', sizeof(slice_written));
	holder = new_file(one, sizeof(one));
	cloned = new_file(slice_bytes, sizeof(slice_bytes));
	slice_file = sim_file_new(mount);
	check(!sim_clone(cloned, 0, slice_file, 0, 2 * PAGE_SIZE), "the clone");
	done = sim_read(slice_file, 0, got, sizeof(got));
	check(done == sizeof(got) && !memcmp(got, slice_bytes, sizeof(got)), "the read: %zd", done);
	check(!sim_punch(cloned, 0, PAGE_SIZE), "the punch");

	snprintf(ino, sizeof(ino), "%lu", file_inode(slice_file)->i_ino);
	sim_param_set("dax_pause_ino", ino);
	snprintf(ino, sizeof(ino), "%lu", PAUSE_READ_MAPPED);
	sim_param_set("dax_pause", ino);
	slice_addr = sim_mmap(slice_file, 0, sizeof(slice_bytes));
	sim_spawn("reader", load_slice, NULL);
	while (!(sim_param_read("dax_paused") & PAUSE_READ_MAPPED))
		msleep(1);
	sim_spawn("writer", write_slice, NULL);
	msleep(100);
	sim_param_set("dax_pause", "0");
	check(!sim_fsync(holder), "the seal");
}

static struct file *killed_file;
static char killed_bytes[4 * PAGE_SIZE];

static void write_killed_file(void *arg)
{
	ssize_t done = sim_write(killed_file, 0, killed_bytes, sizeof(killed_bytes));

	check(done == sizeof(killed_bytes), "the write: %zd", done);
}

/*
 * That a writer killed once its blocks moved, keeping nothing of what they held, still writes them:
 * dax_iomap_iter stopped at the signal, before its copy, which left zeros (patches/0009).
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
	check_file("the write, its task killed", killed_file, killed_bytes, sizeof(killed_bytes));
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

/* A file whose chunk's blocks a write moves, and which its range's own space maps at each move. */
static struct file *sealed_file(char *bytes)
{
	struct file *file;
	char byte;

	sim_param_set("dax_writes_through", "N");
	fill(bytes, BIG_LEN, 1);
	file = new_file(bytes, BIG_FIRST);
	check(sim_write(file, BIG_FIRST, bytes + BIG_FIRST, BIG_LEN - BIG_FIRST) ==
	      BIG_LEN - BIG_FIRST, "the second write");
	check(!sim_fsync(file), "fsync");
	/* The work after a seal over, its MAP of the next staging extent is no test's to fail. */
	msleep(10);
	check(sim_read(file, 0, &byte, 1) == 1, "the first read");
	return file;
}

/* That a MAP failing as a tail page is zeroed, once, is tried again, and truncate says nothing. */
static void map_failed_zeroing(void)
{
	static char bytes[BIG_LEN];
	struct file *file = sealed_file(bytes);
	const size_t size = PAGE_SIZE + 10;
	int err;

	sim_device_fail(VIRTIO_EXTENT_T_MAP, 0, 1);
	err = sim_truncate(file, size);
	check(!err, "a truncate whose zeroing's MAP failed: %d", err);
	check(sim_device_failed() == 1, "no MAP failed");
	check_file("after the truncate", file, bytes, size);
}

/* That MAPs failing for good fail a truncate, and leave no range mapping blocks where they were. */
static void map_failed_zeroing_again(void)
{
	static char bytes[BIG_LEN];
	struct file *file = sealed_file(bytes);
	int err;

	sim_device_fail(VIRTIO_EXTENT_T_MAP, 0, 3);
	err = sim_truncate(file, PAGE_SIZE + 10);
	check(err == -EIO, "a truncate whose MAPs failed: %d", err);
	sim_device_fail(VIRTIO_EXTENT_T_MAP, 0, 0);
	check_file("after the truncate failed", file, bytes, BIG_LEN);
}

static struct file *behind_file;

static void write_part_killed(void *arg)
{
	char some[100];
	ssize_t done;

	memset(some, 'w', sizeof(some));
	current->killed = true;
	done = sim_write(behind_file, PAGE_SIZE + 10, some, sizeof(some));
	check(done == -EINTR, "a write its task was killed in: %zd", done);
}

/*
 * That a range behind Fxfs's blocks isn't used, with no one left to drop it: a write gives up on
 * it, its task killed, which reads then took for what the file holds, until Fxfs freed the blocks.
 */
static void map_failed_killed(void)
{
	static char bytes[BIG_LEN];

	/* The page alone in its extent, which then dies as the page moves, and is freed. */
	behind_file = sealed_file(bytes);
	memset(bytes + PAGE_SIZE, 'p', PAGE_SIZE);
	check(sim_write(behind_file, PAGE_SIZE, bytes + PAGE_SIZE, PAGE_SIZE) == PAGE_SIZE, "a page");
	check(!sim_fsync(behind_file), "fsync");
	msleep(10);
	sim_device_fail(VIRTIO_EXTENT_T_MAP, 0, 1);
	sim_spawn("writer", write_part_killed, NULL);
	msleep(100);
	check(sim_device_failed() == 1, "no MAP failed");
	check_file("after the killed write", behind_file, bytes, BIG_LEN);
}

/* That a write whose MAPs fail for good says so, leaves the file as it was, and reads follow Fxfs. */
static void map_failed_writing(void)
{
	static char bytes[BIG_LEN], written[PAGE_SIZE];
	struct file *file = sealed_file(bytes);
	ssize_t done;

	memset(written, 'w', sizeof(written));
	sim_device_fail(VIRTIO_EXTENT_T_MAP, 0, 3);
	done = sim_write(file, PAGE_SIZE, written, sizeof(written));
	check(done == -EIO, "a write whose MAPs failed: %zd", done);
	sim_device_fail(VIRTIO_EXTENT_T_MAP, 0, 0);
	/* The page as it was: its move commits after its bytes, which never came (TASK-153.5). */
	check_file("after the write failed", file, bytes, BIG_LEN);
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
	/* What it holds, so that a write that fails after its move is one that's tried again. */
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

/*
 * That writes never told to sync, more than the host's memory for open extents holds, each over
 * what the last left dead, make room with a seal, and lose nothing, as xfstests' generic/129 asks
 * (`looptest -t`): the host evicts only what is sealed (TASK-150.20).
 */
static void open_memory_out(void)
{
	static char bytes[25 * PAGE_SIZE], got[25 * PAGE_SIZE];
	struct file *file = sim_file_new(mount);
	int round;

	sim_device_limit_open(3 * PTRS_PER_PTE);
	for (round = 0; round < 1000; round++) {
		fill(bytes, sizeof(bytes), round);
		check(!sim_truncate(file, 0), "truncate %d", round);
		check(sim_write(file, 0, bytes, sizeof(bytes)) == sizeof(bytes), "write %d", round);
		check(sim_read(file, 0, got, sizeof(got)) == sizeof(got), "read %d", round);
		check(!memcmp(got, bytes, sizeof(got)), "round %d reads otherwise", round);
	}
	check(sim_device_open_refused(), "the host refused no MAP");
	check(sim_device_failed() == 0, "a request failed");
	check_file("the last write", file, bytes, sizeof(bytes));
}

/* A file's write, as a task of its own. */
struct reach {
	struct file *file;
	const char *bytes;
	size_t len;
};

static void write_reach(void *arg)
{
	struct reach *write = arg;
	ssize_t done = sim_write(write->file, 0, write->bytes, write->len);

	check(done == write->len, "a write of %zu bytes: %zd", write->len, done);
}

static void seal_reach(void *arg)
{
	check(!sim_fsync(arg), "the seal");
}

static void wait_for(struct task_struct *task)
{
	while (!task->done)
		msleep(1);
}

/*
 * That a region's `mapped` never goes back. Two new files' slices of the staging extent reach at
 * once: the first's MAP, as far as the region's first step, held at the host, the second's, further,
 * and a seal begun, which waits for the first. Without the region's lock, the second MAPs from what
 * the first found mapped, and the first, ending last, says the region maps less than it does: the
 * seal shrinks the region to that, and gives the window past it back, under the second's slice. The
 * next staging extent's region takes it, where the first's write, sealed before its copy, moves its
 * page: the second file reads the first's bytes (TASK-44.19's map_to_unlocked, which no guest suite
 * found, nor 3,000 seeds of interleave and failures).
 */
static void reaches_at_once(void)
{
	static char near[PAGE_SIZE], far[40 * PAGE_SIZE], got[sizeof(far)];
	struct reach first = { sim_file_new(mount), near, sizeof(near) };
	struct reach second = { sim_file_new(mount), far, sizeof(far) };
	struct file *sealed = sim_file_new(mount);
	struct task_struct *nearer, *further, *sealer;
	size_t page;
	ssize_t done;
	u64 held;

	fill(near, sizeof(near), 1);
	fill(far, sizeof(far), 2);
	sim_device_hold_map();
	nearer = sim_spawn("nearer", write_reach, &first);
	while (!(held = sim_device_held_map()))
		msleep(1);
	/* The region's first MAP, FXFS_DAX_REGION_FIRST's pages, short of the second slice's end. */
	check(held == 16, "the MAP held is of %llu pages", held);
	further = sim_spawn("further", write_reach, &second);
	/* As far as it goes: its write's end, or, as the region is locked, its MAP's beginning. */
	msleep(100);
	sealer = sim_spawn("sealer", seal_reach, sealed);
	while (!fxfs_sim_sealing(mount->info->fs))
		msleep(1);
	sim_device_release_map();
	wait_for(nearer);
	wait_for(further);
	wait_for(sealer);
	done = sim_read(second.file, 0, got, sizeof(got));
	check(done == sizeof(got), "the further file's read: %zd", done);
	for (page = 0; page < ARRAY_SIZE(far) / PAGE_SIZE; page++) {
		check(!memcmp(got + page * PAGE_SIZE, far + page * PAGE_SIZE, PAGE_SIZE),
		      "the further file's page %zu reads %s", page,
		      !memcmp(got + page * PAGE_SIZE, near, PAGE_SIZE) ? "the nearer file's bytes" :
		      "bytes of neither file");
	}
	check_file("the further slice", second.file, far, sizeof(far));
	check_file("the nearer slice", first.file, near, sizeof(near));
}

static const struct {
	const char *name;
	void (*run)(void);
} scenarios[] = {
#define SCENARIO(name) { #name, name }
	SCENARIO(short_copy),
	SCENARIO(short_copy_pages),
	SCENARIO(same_page),
	SCENARIO(append_stopped_short_in_a_page),
	SCENARIO(store_past_end_then_written),
	SCENARIO(write_past_end_copies_nothing),
	SCENARIO(clone_past_a_writable_region),
	SCENARIO(clone_pieces_past_a_writable_region),
	SCENARIO(slice_refuses_a_piece),
	SCENARIO(write_killed),
	SCENARIO(unmap_failed),
	SCENARIO(map_failed_zeroing),
	SCENARIO(map_failed_zeroing_again),
	SCENARIO(map_failed_killed),
	SCENARIO(map_failed_writing),
	SCENARIO(allocations_failed),
	SCENARIO(open_memory_out),
	SCENARIO(reaches_at_once),
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
			sim_spawn(scenarios[i].name, scenario, scenarios[i].run);
			sim_run();
			sim_log("%s: passed\n", scenarios[i].name);
			return 0;
		}
	}
	return argc < 2 ? 0 : 1;
}
