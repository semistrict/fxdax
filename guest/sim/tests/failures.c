// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * What tests/interleave leaves out: things that fail. Tasks write, map, truncate and clone their
 * files, from memory that ends part way through, and from the files' own mappings, as allocations
 * and the device's MAPs and UNMAPs fail, one in so many, the seed's.
 *
 *   failures SEED [OPERATIONS] [-v] [-W INO PAGE] [-a ADDRESS]
 *
 * An operation that fails may have done anything to the bytes it was to change, which the task
 * then knows nothing of; one that nothing failed under has done exactly what it says, and what a
 * task knows, it must read, through read(2) and its mapping, whatever else fails meanwhile. And
 * every so often, with nothing failing, read(2) must read what Fxfs's extents hold, whatever the
 * task knows: a range left behind Fxfs's blocks reads where they were. A task killed in a write
 * leaves each byte as it was or as written, zeros no part of either. The filesystem then unmounts.
 */

#include <sim/sim.h>

#define WINDOW_LEN (64UL << 20)
#define WORKERS 2
/* Past a chunk, so that a range of its own maps it, and it holds an extent of its own. */
#define BIG_LEN (3UL << 20)
#define SMALL_MAX (32 * PAGE_SIZE)
/* What a write copies at most, and how often, in operations, all of it is checked. */
#define WRITE_MAX (3 * PAGE_SIZE)
#define ROUND 16
/* One allocation, and one MAP or UNMAP, in so many fails. */
#define FAIL_ODDS 25
/* The host's memory for open extents, a run in four: what it refuses a seal makes room for. */
#define OPEN_LIMIT PTRS_PER_PTE

#define check(cond, ...) do { if (!(cond)) sim_panic(__VA_ARGS__); } while (0)

/* A file, with its bytes as they should read, and which of them it knows. */
struct tracked {
	struct file *file;
	char *bytes;
	unsigned long *unknown;
	unsigned long addr;
	loff_t size, max;
};

struct worker {
	int id;
	struct tracked big, small;
	struct file *clone;
};

static struct worker workers[WORKERS];
static struct sim_mount *mount;
static int writers_left;
static unsigned long operations = 200;
static bool verbose;
/* A file's bytes to copy from, and addresses at which so many of them are left, the last byte's. */
static char source_bytes[WRITE_MAX];
static unsigned long source_end;
static struct file *source_file;

#define operation(worker, ...) do {					\
	if (verbose) {							\
		sim_log("op: worker %d, %llu ns: ", (worker)->id, ktime_get_ns()); \
		sim_log(__VA_ARGS__);					\
	}								\
} while (0)

static u64 between(u64 low, u64 high)
{
	return low + sim_random() % (high - low + 1);
}

static void new_bytes(char *buf, size_t len)
{
	u64 value = sim_random();
	size_t i;

	for (i = 0; i < len; i++) {
		if (!(i % 8))
			value = sim_random();
		buf[i] = value >> (i % 8 * 8);
	}
}

/* How many things have failed: an operation is exact if this stays. */
static u64 failures(void)
{
	return sim_allocs_failed() + sim_device_failed();
}

static void forget(struct tracked *file, loff_t at, size_t len)
{
	size_t i;

	for (i = 0; i < len; i++)
		set_bit(at + i, file->unknown);
}

static void know(struct tracked *file, loff_t at, size_t len)
{
	size_t i;

	for (i = 0; i < len; i++)
		clear_bit(at + i, file->unknown);
}

static void check_known(const char *what, struct tracked *file, const char *got, loff_t at,
			size_t len)
{
	size_t i;
	char held;

	for (i = 0; i < len; i++) {
		if (got[i] == file->bytes[at + i] || test_bit(at + i, file->unknown))
			continue;
		fxfs_sim_read(mount->info->fs, file_inode(file->file)->i_ino, at + i, &held, 1);
		sim_log("failures: Fxfs's extents hold %#x there\n", (u8)held);
		sim_panic("%s at %lld of ino %lu: byte %zu is %#x, not %#x", what, at,
			  file_inode(file->file)->i_ino, i, (u8)got[i], (u8)file->bytes[at + i]);
	}
}

/* The outcome of a write of `len` bytes from `source` at `at`, which `done` says. */
static void written(struct tracked *file, loff_t at, const char *source, size_t len, ssize_t done,
		    size_t readable, bool failed)
{
	size_t wrote = done > 0 ? done : 0;

	memmove(file->bytes + at, source, wrote);
	know(file, at, wrote);
	if (wrote)
		file->size = max_t(loff_t, file->size, at + wrote);
	if (!failed) {
		check(done == min(len, readable), "a write of %zu from %zu readable: %zd", len,
		      readable, done);
		return;
	}
	/* EFAULT of memory a fault failed for, a mapping's. */
	if (done < 0)
		check(done == -EIO || done == -ENOMEM || done == -EFAULT, "a write: %zd", done);
	/* What it didn't write it may have moved, keeping nothing. */
	if (wrote < len && at + wrote < file->size)
		forget(file, at + wrote, min_t(loff_t, len - wrote, file->size - at - wrote));
}

static void op_write(struct worker *worker, struct tracked *file)
{
	/* Past the end at times, which what is between must then read as zeros. */
	loff_t at = between(0, min_t(loff_t, file->size + 2 * PAGE_SIZE, file->max - 1));
	size_t len = between(1, min_t(u64, file->max - at, WRITE_MAX));
	char *buf = kmalloc(len, GFP_KERNEL);
	u64 before = failures();
	ssize_t done;

	operation(worker, "write %lld+%zu, size %lld\n", at, len, file->size);
	new_bytes(buf, len);
	done = sim_write(file->file, at, buf, len);
	written(file, at, buf, len, done, len, failures() != before);
	kfree(buf);
}

/* A write from memory that ends part way through it. */
static void op_short_write(struct worker *worker, struct tracked *file)
{
	loff_t at = between(0, min_t(loff_t, file->size, file->max - 1));
	size_t len = between(2, min_t(u64, file->max - at, WRITE_MAX));
	size_t readable = between(1, len - 1);
	u64 before = failures();
	ssize_t done;

	operation(worker, "write %lld+%zu, %zu readable, size %lld\n", at, len, readable, file->size);
	done = sim_write_user(file->file, at, source_end - readable, len);
	written(file, at, source_bytes + WRITE_MAX - readable, len, done, readable,
		failures() != before);
}

/* A write by a task killed as it begins: each byte what it held, or what the write wrote. */
static void op_killed_write(struct worker *worker, struct tracked *file)
{
	loff_t at = between(0, min_t(loff_t, file->size, file->max - 1));
	size_t len = between(1, min_t(u64, file->max - at, WRITE_MAX)), i;
	char *buf = kmalloc(len, GFP_KERNEL), *got = kmalloc(len, GFP_KERNEL);
	u64 before = failures();
	ssize_t done;
	size_t held;

	operation(worker, "write %lld+%zu, killed, size %lld\n", at, len, file->size);
	new_bytes(buf, len);
	current->killed = true;
	done = sim_write(file->file, at, buf, len);
	current->killed = false;
	file->size = max_t(loff_t, file->size, at + max_t(ssize_t, done, 0));
	held = min_t(loff_t, len, file->size - at);
	if (sim_read(file->file, at, got, held) != held || failures() != before) {
		forget(file, at, held);
		held = 0;
	}
	for (i = 0; i < held; i++) {
		if (got[i] != buf[i] && got[i] != file->bytes[at + i] &&
		    !test_bit(at + i, file->unknown))
			sim_panic("a killed write at %lld, which said %zd: byte %zu is %#x, not %#x or %#x",
				  at, done, i, (u8)got[i], (u8)file->bytes[at + i], (u8)buf[i]);
		file->bytes[at + i] = got[i];
		clear_bit(at + i, file->unknown);
	}
	kfree(buf);
	kfree(got);
}

/* A write from a mapping of the file, of bytes it writes itself, or of others. */
static void op_self_write(struct worker *worker, struct tracked *file)
{
	size_t len = between(1, WRITE_MAX);
	loff_t from, to;
	u64 before = failures();
	bool unknown;
	char *held;
	ssize_t done;

	if (file->size < len + 1)
		return;
	from = between(0, file->size - len);
	/* Where bytes overlap, a copy's order says what they get, but for a page on itself. */
	to = between(0, 1) ? from : between(from + len, max_t(loff_t, from + len, file->size - len));
	if (to + len > file->max)
		return;
	operation(worker, "write %lld+%zu from the mapping at %lld, size %lld\n", to, len, from,
		  file->size);
	held = kmalloc(len, GFP_KERNEL);
	memcpy(held, file->bytes + from, len);
	unknown = find_next_bit(file->unknown, from + len, from) != from + len;
	done = sim_write_user(file->file, to, file->addr + from, len);
	written(file, to, held, len, done, len, failures() != before);
	/* What it copied of bytes it didn't know, it doesn't know. */
	if (done > 0 && unknown)
		forget(file, to, done);
	kfree(held);
}

static void op_read(struct worker *worker, struct tracked *file)
{
	size_t len;
	loff_t at;
	char *buf;
	ssize_t done;

	if (!file->size)
		return;
	at = between(0, file->size - 1);
	len = between(1, min_t(u64, file->size - at, WRITE_MAX));
	operation(worker, "read %lld+%zu, size %lld\n", at, len, file->size);
	buf = kmalloc(len, GFP_KERNEL);
	done = sim_read(file->file, at, buf, len);
	check(done <= (ssize_t)len && done >= -ENOMEM, "a read: %zd", done);
	if (done > 0)
		check_known("a read", file, buf, at, done);
	kfree(buf);
}

static void op_load(struct worker *worker, struct tracked *file)
{
	size_t len;
	loff_t at;
	char *buf;

	if (!file->size)
		return;
	at = between(0, file->size - 1);
	len = between(1, min_t(u64, file->size - at, PAGE_SIZE));
	operation(worker, "load %lld+%zu, size %lld\n", at, len, file->size);
	buf = kmalloc(len, GFP_KERNEL);
	/* A fault may fail: a SIGBUS is what the task gets. */
	if (!sim_user_load(file->addr + at, buf, len))
		check_known("a load", file, buf, at, len);
	kfree(buf);
}

/* A store past the file's end, in its last page, which no read of the file is to find. */
static void op_store_past_end(struct worker *worker, struct tracked *file)
{
	size_t room = -file->size & (PAGE_SIZE - 1), len;
	char *buf;

	if (!room)
		return;
	len = between(1, room);
	operation(worker, "store %lld+%zu, past the end\n", file->size, len);
	buf = kmalloc(len, GFP_KERNEL);
	new_bytes(buf, len);
	sim_user_store(file->addr + file->size, buf, len);
	kfree(buf);
}

static void op_store(struct worker *worker, struct tracked *file)
{
	size_t len;
	loff_t at;
	char *buf;

	if (!file->size)
		return;
	at = between(0, file->size - 1);
	len = between(1, min_t(u64, file->size - at, 64));
	operation(worker, "store %lld+%zu, size %lld\n", at, len, file->size);
	buf = kmalloc(len, GFP_KERNEL);
	new_bytes(buf, len);
	if (!sim_user_store(file->addr + at, buf, len)) {
		memcpy(file->bytes + at, buf, len);
		know(file, at, len);
	} else {
		forget(file, at, len);
	}
	kfree(buf);
}

static void op_truncate(struct worker *worker, struct tracked *file)
{
	loff_t size = between(0, file->max);
	u64 before = failures();
	loff_t old = file->size;
	int err;

	operation(worker, "truncate to %lld, size %lld\n", size, file->size);
	err = sim_truncate(file->file, size);
	if (err) {
		check(failures() != before && (err == -EIO || err == -ENOMEM), "a truncate: %d", err);
		/* Its zeroing moved the page it was to zero. */
		forget(file, round_down(min(size, old), PAGE_SIZE),
		       min_t(loff_t, PAGE_SIZE, old - round_down(min(size, old), PAGE_SIZE)));
		return;
	}
	if (size < old)
		memset(file->bytes + size, 0, old - size);
	know(file, min(size, old), max(size, old) - min(size, old));
	file->size = size;
}

static void op_punch(struct worker *worker, struct tracked *file)
{
	loff_t start, end;
	u64 before = failures();
	int err;

	if (!file->size)
		return;
	start = between(0, file->size - 1);
	end = between(start + 1, min_t(loff_t, file->size, start + 8 * PAGE_SIZE));
	operation(worker, "punch %lld to %lld, size %lld\n", start, end, file->size);
	err = sim_punch(file->file, start, end);
	if (err) {
		check(failures() != before && (err == -EIO || err == -ENOMEM), "a punch: %d", err);
		forget(file, start, end - start);
		return;
	}
	memset(file->bytes + start, 0, end - start);
	know(file, start, end - start);
}

/* Clones part of the file, so that its blocks are shared, some of them in open extents. */
static void op_clone(struct worker *worker, struct tracked *file)
{
	pgoff_t pages = file->size >> PAGE_SHIFT, page, count;

	if (!pages)
		return;
	page = between(0, pages - 1);
	count = between(1, min_t(pgoff_t, pages - page, 8));
	operation(worker, "clone page %lu+%lu, size %lld\n", page, count, file->size);
	/* A failure's its own: a file's blocks stay as they are. */
	if (!sim_clone(file->file, (loff_t)page << PAGE_SHIFT, worker->clone,
		       (loff_t)page << PAGE_SHIFT, (loff_t)count << PAGE_SHIFT)) {
		char *got = kmalloc(count << PAGE_SHIFT, GFP_KERNEL);
		ssize_t done = sim_read(worker->clone, (loff_t)page << PAGE_SHIFT, got,
					count << PAGE_SHIFT);

		if (done > 0)
			check_known("a read of a clone", file, got, (loff_t)page << PAGE_SHIFT, done);
		kfree(got);
	}
	if (between(0, 1))
		sim_truncate(worker->clone, 0);
}

/* Nothing fails while any task checks what it has. */
static void set_failing(bool on)
{
	static int checking;

	checking += on ? -1 : 1;
	sim_alloc_fail_odds(checking ? 0 : FAIL_ODDS);
	sim_device_fail_odds(checking ? 0 : FAIL_ODDS);
}

/* With nothing failing, that the file reads as it knows it, and as Fxfs's extents hold it. */
static void check_whole(struct worker *worker, struct tracked *file)
{
	char *got = kmalloc(file->max, GFP_KERNEL), *held = kmalloc(file->max, GFP_KERNEL);
	ssize_t done = file->size ? sim_read(file->file, 0, got, file->size) : 0;
	loff_t i;

	check(done == file->size, "reading it all: %zd of %lld", done, file->size);
	check_known("reading it all", file, got, 0, file->size);
	check(!sim_fsync(file->file), "fsync");
	fxfs_sim_read(mount->info->fs, file_inode(file->file)->i_ino, 0, held, file->size);
	for (i = 0; i < file->size; i++) {
		if (got[i] == held[i])
			continue;
		sim_panic("worker %d: ino %lu at %lld: read(2) has %#x, and Fxfs's extents %#x",
			  worker->id, file_inode(file->file)->i_ino, i, (u8)got[i], (u8)held[i]);
	}
	/* What it can't know any more, it knows now. */
	memcpy(file->bytes, got, file->size);
	know(file, 0, file->size);
	kfree(got);
	kfree(held);
}

static void worker_task(void *arg)
{
	struct worker *worker = arg;
	unsigned long i;

	for (i = 0; i < operations; i++) {
		struct tracked *file = between(0, 1) ? &worker->big : &worker->small;

		if (!(i % ROUND)) {
			set_failing(false);
			check_whole(worker, &worker->big);
			check_whole(worker, &worker->small);
			set_failing(true);
		}
		switch (between(0, 14)) {
		case 0 ... 2:
			op_write(worker, file);
			break;
		case 13:
			op_killed_write(worker, file);
			break;
		case 14:
			op_store_past_end(worker, file);
			break;
		case 3 ... 4:
			op_short_write(worker, file);
			break;
		case 5 ... 6:
			op_self_write(worker, file);
			break;
		case 7:
			op_read(worker, file);
			break;
		case 8:
			op_load(worker, file);
			break;
		case 9:
			op_store(worker, file);
			break;
		case 10:
			op_truncate(worker, file);
			break;
		case 11:
			op_punch(worker, file);
			break;
		default:
			op_clone(worker, file);
			break;
		}
	}
	writers_left--;
}

/* Seals as a journal does, whenever it likes. */
static void syncer_task(void *arg)
{
	while (READ_ONCE(writers_left)) {
		check(!sim_fsync(workers[0].big.file), "fsync");
		msleep(between(0, 2));
	}
}

static void forget_file(struct tracked *file)
{
	sim_munmap(file->addr);
	sim_file_evict(file->file);
}

/* With what it failed to UNMAP kept from use all along. */
static void unmount_task(void *arg)
{
	int i;

	sim_device_fail_odds(FAIL_ODDS);
	for (i = 0; i < WORKERS; i++) {
		forget_file(&workers[i].big);
		forget_file(&workers[i].small);
		sim_file_evict(workers[i].clone);
	}
	sim_munmap(source_end - sizeof(source_bytes));
	sim_file_evict(source_file);
	sim_mount_free(mount);
}

static void track(struct tracked *file, loff_t size, loff_t max)
{
	ssize_t done;

	file->file = sim_file_new(mount);
	file->max = max;
	file->size = size;
	file->bytes = kzalloc(max, GFP_KERNEL);
	file->unknown = kzalloc(BITS_TO_LONGS(max) * sizeof(long), GFP_KERNEL);
	new_bytes(file->bytes, size);
	done = sim_write(file->file, 0, file->bytes, size);
	check(done == size, "the first write: %zd", done);
	file->addr = sim_mmap(file->file, 0, max);
}

static void setup(void)
{
	int i;

	mount = sim_mount_new();
	/* Not dax_check_moves: stores past a file's end aren't the file's, which it takes them for. */
	sim_param_set("dax_check_seals", "1");
	new_bytes(source_bytes, sizeof(source_bytes));
	source_file = sim_file_new(mount);
	check(sim_write(source_file, 0, source_bytes, sizeof(source_bytes)) == sizeof(source_bytes),
	      "the source");
	source_end = sim_mmap(source_file, 0, sizeof(source_bytes)) + sizeof(source_bytes);
	for (i = 0; i < WORKERS; i++) {
		workers[i].id = i;
		track(&workers[i].big, BIG_LEN, BIG_LEN);
		track(&workers[i].small, 4 * PAGE_SIZE, SMALL_MAX);
		workers[i].clone = sim_file_new(mount);
	}
	check(!sim_fsync(workers[0].big.file), "the first seal");
	writers_left = WORKERS;
}

int main(int argc, char **argv)
{
	unsigned long long seed = 0;
	unsigned long watched_address = 0;
	int i;

	if (argc < 2 || sscanf(argv[1], "%llu", &seed) != 1)
		sim_panic("usage: failures SEED [OPERATIONS] [-v]");
	if (argc > 2 && argv[2][0] != '-')
		sscanf(argv[2], "%lu", &operations);
	sim_init(seed);
	sim_device_init(WINDOW_LEN);
	if (!(seed % 4))
		sim_device_limit_open(OPEN_LIMIT);
	for (i = 2; i < argc; i++) {
		unsigned long long ino, page;

		if (!strcmp(argv[i], "-v"))
			verbose = true;
		else if (!strcmp(argv[i], "-a") && i + 1 < argc)
			sscanf(argv[i + 1], "%lx", &watched_address);
		else if (!strcmp(argv[i], "-W") && i + 2 < argc &&
			 sscanf(argv[i + 1], "%llu", &ino) == 1 &&
			 sscanf(argv[i + 2], "%llu", &page) == 1)
			fxfs_sim_watch(ino, page);
	}
	setup();
	if (watched_address)
		sim_mm_watch(watched_address);
	sim_spawn("syncer", syncer_task, NULL);
	for (i = 0; i < WORKERS; i++)
		sim_spawn(i ? "worker1" : "worker0", worker_task, &workers[i]);
	sim_run();
	sim_alloc_fail_odds(0);
	sim_device_fail_odds(0);
	for (i = 0; i < WORKERS; i++) {
		check_whole(&workers[i], &workers[i].big);
		check_whole(&workers[i], &workers[i].small);
	}
	sim_spawn("unmount", unmount_task, NULL);
	sim_run();
	sim_log("failures: seed %llu passed: %llu points, %llu allocations and %llu requests failed, trace %llx\n",
		seed, sim_points(), sim_allocs_failed(), sim_device_failed(), sim_trace_hash());
	return 0;
}
