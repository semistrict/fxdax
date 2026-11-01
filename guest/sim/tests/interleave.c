// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * Tasks writing, mapping, sealing and truncating at once, their interleaving the seed's: the same
 * seed plays the same run, whose trace hash says so.
 *
 *   interleave SEED [OPERATIONS] [-s] [-t] [-T] [-v] [-I] [-N] [-w WINDOW_PAGE] [-a ADDRESS] [-W INO PAGE]
 *
 * -s runs dax_shadow too, -t logs each scheduling point, -T has dax.c's trace of the page that
 * fails logged, -w logs the window page's MAPs, -a the address's PTE, and -W what Fxfs does to the
 * file's page; -v each operation. -I leaves dax.c's fault injection off, and -N the operations that
 * take a file's own mapping for their memory.
 *
 * Tasks share files without sharing bytes, so that each knows what every byte it owns should hold
 * though pages move under all of them at once: each owns a slot of every page of the shared file,
 * and a private file. What it reads must be what it wrote; once all end, a seal's extents must
 * hold all of it. dax.c's own diagnostics are on, and each of its WARNs fails the run.
 */

#include <sim/sim.h>

#define WINDOW_LEN (64UL << 20)
#define WORKERS 3
#define SHARED_PAGES 64
/* Past a PMD, so that a read fault can map 2 MiB of it at once. */
#define BIG_PAGES (2 * PTRS_PER_PTE)
#define PRIVATE_MAX (32 * PAGE_SIZE)
/* Small enough that they are slices of the staging extent's region, which they share. */
#define SMALL_FILES 4
#define SMALL_PAGES 4
#define HUGE_LEN (2 * PMD_SIZE)
#define SLOT 32

#define check(cond, ...) do { if (!(cond)) sim_panic(__VA_ARGS__); } while (0)

struct worker {
	int id;
	/* What it wrote in its slot of each page of the shared and big files. */
	char shared[SHARED_PAGES][SLOT];
	char big[BIG_PAGES][SLOT];
	char small[SMALL_FILES][SMALL_PAGES][SLOT];
	/* Its private file, as it should read. */
	struct file *file;
	char *private;
	loff_t size;
	/* A mapping of its private file, which writes copy from and reads copy to. */
	unsigned long addr;
	/* A file of a sealed PMD's data, written again each cycle (huge_cycle). */
	struct file *huge_file;
	unsigned long huge_addr;
};

static struct worker workers[WORKERS];
static struct sim_mount *mount;
static struct file *shared_file, *big_file, *small_files[SMALL_FILES];
static unsigned long shared_addr, big_addr, small_addrs[SMALL_FILES];
static int writers_left;
static unsigned long operations = 200;
/*
 * Whether every write fault seals: tasks then undo each other's faults for ever, each seal making
 * read-only the page another's copy into a mapping needs, which fails it, as it should.
 */
static bool fault_seals;
/* How many cycles a PMD did map, for the log: the path needs reaching to mean anything. */
static unsigned long huge_mapped;
/* Whether dax.c traces what happens to pages, and whether each operation is logged. */
static bool tracing, verbose;

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

static void check_bytes(const char *what, const struct worker *worker, struct file *file,
			const char *got, const char *expected, size_t len, loff_t at)
{
	char page[48], held;
	size_t i;

	for (i = 0; i < len; i++) {
		if (got[i] == expected[i])
			continue;
		fxfs_sim_read(mount->info->fs, file_inode(file)->i_ino, at + i, &held, 1);
		sim_log("interleave: page %llu of ino %lu: Fxfs's extents hold %#x there\n",
			(at + i) >> PAGE_SHIFT, file_inode(file)->i_ino, (u8)held);
		if (tracing) {
			snprintf(page, sizeof(page), "%lu %llu", file_inode(file)->i_ino,
				 (at + i) >> PAGE_SHIFT);
			sim_param_set("dax_trace_dump", page);
		}
		sim_panic("%s of worker %d at %lld: byte %zu is %#x, not %#x", what, worker->id, at,
			  i, (u8)got[i], (u8)expected[i]);
	}
}

/* A slot of a page of a file every worker has a slot of each page of. */
static void slot_op(struct worker *worker, struct file *file, unsigned long addr, char *slots,
		    unsigned long pages)
{
	unsigned long page = between(0, pages - 1);
	loff_t at = ((loff_t)page << PAGE_SHIFT) + worker->id * SLOT;
	char *mine = slots + page * SLOT, buf[SLOT];
	ssize_t done;
	int err;

	switch (between(0, 3)) {
	case 0:
		new_bytes(buf, SLOT);
		memcpy(mine, buf, SLOT);
		done = sim_write(file, at, buf, SLOT);
		check(done == SLOT, "write: %zd", done);
		break;
	case 1:
		new_bytes(buf, SLOT);
		memcpy(mine, buf, SLOT);
		err = sim_user_store(addr + at, buf, SLOT);
		check(!err, "a store at page %lu of worker %d: %d", page, worker->id, err);
		break;
	case 2:
		done = sim_read(file, at, buf, SLOT);
		check(done == SLOT, "read: %zd", done);
		check_bytes("read", worker, file, buf, mine, SLOT, at);
		break;
	default:
		err = sim_user_load(addr + at, buf, SLOT);
		check(!err, "a load at page %lu of worker %d: %d", page, worker->id, err);
		if (memcmp(buf, mine, SLOT))
			sim_mm_describe(addr + at);
		check_bytes("load", worker, file, buf, mine, SLOT, at);
		break;
	}
}

static void private_write(struct worker *worker)
{
	loff_t at = between(0, min_t(loff_t, worker->size, PRIVATE_MAX - 1));
	size_t len = between(1, min_t(u64, PRIVATE_MAX - at, 3 * PAGE_SIZE));
	ssize_t done;

	operation(worker, "write %lld+%zu, size %lld\n", at, len, worker->size);
	new_bytes(worker->private + at, len);
	done = sim_write(worker->file, at, worker->private + at, len);
	check(done == len, "a private write: %zd of %zu", done, len);
	worker->size = max_t(loff_t, worker->size, at + len);
}

static void private_read(struct worker *worker)
{
	size_t len;
	loff_t at;
	char *buf;
	ssize_t done;

	if (!worker->size)
		return;
	at = between(0, worker->size - 1);
	len = between(1, min_t(u64, worker->size - at, 3 * PAGE_SIZE));
	buf = kmalloc(len, GFP_KERNEL);
	done = sim_read(worker->file, at, buf, len);
	check(done == len, "a private read: %zd of %zu", done, len);
	check_bytes("a private read", worker, worker->file, buf, worker->private + at, len, at);
	kfree(buf);
}

/*
 * A write from a mapping of the file it writes, past its end if it likes: the fault on its own
 * source remaps the chunk as the write drops it (xfstests generic/638, TASK-147).
 */
static void private_self_write(struct worker *worker)
{
	size_t len;
	loff_t from, to;
	ssize_t done;

	if (worker->size < 4 * PAGE_SIZE || fault_seals)
		return;
	/* Not in a page of its target, which it unmaps each time: it would fail, as dax.c says. */
	len = between(1, PAGE_SIZE);
	from = between(0, worker->size - 3 * PAGE_SIZE);
	to = between(round_up(from + len, PAGE_SIZE),
		     min_t(loff_t, worker->size, PRIVATE_MAX - len));
	operation(worker, "write %lld+%zu from the mapping at %lld, size %lld\n", to, len, from,
		  worker->size);
	memcpy(worker->private + to, worker->private + from, len);
	done = sim_write_user(worker->file, to, worker->addr + from, len);
	check(done == len, "a write from its own mapping: %zd of %zu", done, len);
	worker->size = max_t(loff_t, worker->size, to + len);
}

/* A read into a mapping of the file it reads, whose fault may wait for the iomap of the read. */
static void private_self_read(struct worker *worker)
{
	size_t len;
	loff_t from, to;
	ssize_t done;

	if (worker->size < 4 * PAGE_SIZE || fault_seals)
		return;
	len = between(1, PAGE_SIZE);
	from = between(0, worker->size - 3 * PAGE_SIZE);
	to = between(round_up(from + len, PAGE_SIZE), worker->size - len);
	operation(worker, "read %lld+%zu into the mapping at %lld, size %lld\n", from, len, to,
		  worker->size);
	memcpy(worker->private + to, worker->private + from, len);
	done = sim_read_user(worker->file, from, worker->addr + to, len);
	check(done == len, "a read into its own mapping: %zd of %zu", done, len);
}

/* A load through the mapping must see what read(2) does. */
static void private_load(struct worker *worker)
{
	size_t len;
	loff_t at;
	char *buf;
	int err;

	if (!worker->size)
		return;
	at = between(0, worker->size - 1);
	len = between(1, min_t(u64, worker->size - at, 3 * PAGE_SIZE));
	operation(worker, "load %lld+%zu, size %lld\n", at, len, worker->size);
	buf = kmalloc(len, GFP_KERNEL);
	err = sim_user_load(worker->addr + at, buf, len);
	check(!err, "a private load: %d", err);
	check_bytes("a private load", worker, worker->file, buf, worker->private + at, len, at);
	kfree(buf);
}

static void private_truncate(struct worker *worker)
{
	loff_t size = between(0, PRIVATE_MAX / 2);
	int err;

	operation(worker, "truncate to %lld, size %lld\n", size, worker->size);
	if (size < worker->size)
		memset(worker->private + size, 0, worker->size - size);
	err = sim_truncate(worker->file, size);
	check(!err, "a truncate: %d", err);
	worker->size = size;
}

static void private_punch(struct worker *worker)
{
	loff_t start, end;
	int err;

	if (!worker->size)
		return;
	start = between(0, worker->size - 1);
	end = between(start + 1, worker->size);
	operation(worker, "punch %lld to %lld, size %lld\n", start, end, worker->size);
	memset(worker->private + start, 0, end - start);
	err = sim_punch(worker->file, start, end);
	check(!err, "a punch: %d", err);
}

/*
 * A chunk mapped by a PMD, whose page a store then moves out of the row: its other pages must fault
 * back in, each read what it held (TASK-123). Written whole, so that one extent holds it, sealed,
 * larger than a chunk, so that its chunks are ranges of their own, not slices; truncated first, so
 * that no entries of its pages keep a PMD's fault out; and mapped anew, as a PTE table left under
 * the PMD would.
 */
static void huge_cycle(struct worker *worker)
{
	static char data[WORKERS][HUGE_LEN];
	char *mine = data[worker->id], buf[SLOT];
	unsigned long moved = between(0, PTRS_PER_PTE - 1), other;
	ssize_t done;
	int err, i;

	operation(worker, "a PMD's cycle, moving page %lu\n", moved);
	sim_munmap(worker->huge_addr);
	worker->huge_addr = sim_mmap(worker->huge_file, 0, HUGE_LEN);
	err = sim_truncate(worker->huge_file, 0);
	check(!err, "a truncate: %d", err);
	new_bytes(mine, HUGE_LEN);
	done = sim_write(worker->huge_file, 0, mine, HUGE_LEN);
	check(done == HUGE_LEN, "a whole file's write: %zd", done);
	err = sim_fsync(worker->huge_file);
	check(!err, "fsync: %d", err);
	err = sim_user_load(worker->huge_addr, buf, SLOT);
	check(!err, "a load of a PMD: %d", err);
	check_bytes("a load of a PMD", worker, worker->huge_file, buf, mine, SLOT, 0);
	huge_mapped += sim_mm_is_huge(worker->huge_addr);
	new_bytes(buf, SLOT);
	memcpy(mine + moved * PAGE_SIZE, buf, SLOT);
	err = sim_user_store(worker->huge_addr + moved * PAGE_SIZE, buf, SLOT);
	check(!err, "a store into a PMD: %d", err);
	for (i = 0; i < 8; i++) {
		other = between(0, PTRS_PER_PTE - 1);
		err = sim_user_load(worker->huge_addr + other * PAGE_SIZE, buf, SLOT);
		check(!err, "a load of page %lu of a broken PMD: %d", other, err);
		check_bytes("a load of a broken PMD", worker, worker->huge_file, buf,
			    mine + other * PAGE_SIZE, SLOT, other * PAGE_SIZE);
	}
}

static void worker_task(void *arg)
{
	struct worker *worker = arg;
	unsigned long i;

	for (i = 0; i < operations; i++) {
		unsigned long small = between(0, SMALL_FILES - 1);

		switch (between(0, 16)) {
		case 0 ... 3:
			slot_op(worker, shared_file, shared_addr, &worker->shared[0][0],
				SHARED_PAGES);
			break;
		case 13 ... 15:
			slot_op(worker, small_files[small], small_addrs[small],
				&worker->small[small][0][0], SMALL_PAGES);
			break;
		case 16:
			if (!between(0, 3))
				huge_cycle(worker);
			break;
		case 4 ... 5:
			slot_op(worker, big_file, big_addr, &worker->big[0][0], BIG_PAGES);
			break;
		case 6:
			private_write(worker);
			break;
		case 7:
			private_read(worker);
			break;
		case 8:
			private_self_write(worker);
			break;
		case 9:
			private_self_read(worker);
			break;
		case 10:
			private_truncate(worker);
			break;
		case 11:
			private_load(worker);
			break;
		default:
			private_punch(worker);
			break;
		}
	}
	writers_left--;
}

/* Seals as a journal does, whenever it likes. */
static void syncer_task(void *arg)
{
	int err;

	while (READ_ONCE(writers_left)) {
		err = sim_fsync(shared_file);
		check(!err, "fsync: %d", err);
		msleep(between(0, 2));
	}
}

static void check_extents(const struct worker *worker)
{
	struct fxfs *fs = mount->info->fs;
	char buf[SLOT], *whole;
	unsigned long page;
	int i;

	for (page = 0; page < SHARED_PAGES; page++) {
		fxfs_sim_read(fs, file_inode(shared_file)->i_ino,
			      (page << PAGE_SHIFT) + worker->id * SLOT, buf, SLOT);
		check_bytes("the shared file's extents", worker, shared_file, buf,
			    worker->shared[page], SLOT, page << PAGE_SHIFT);
	}
	for (page = 0; page < BIG_PAGES; page++) {
		fxfs_sim_read(fs, file_inode(big_file)->i_ino,
			      (page << PAGE_SHIFT) + worker->id * SLOT, buf, SLOT);
		check_bytes("the big file's extents", worker, big_file, buf, worker->big[page],
			    SLOT, page << PAGE_SHIFT);
	}
	for (i = 0; i < SMALL_FILES; i++) {
		for (page = 0; page < SMALL_PAGES; page++) {
			fxfs_sim_read(fs, file_inode(small_files[i])->i_ino,
				      (page << PAGE_SHIFT) + worker->id * SLOT, buf, SLOT);
			check_bytes("a small file's extents", worker, small_files[i], buf,
				    worker->small[i][page], SLOT, page << PAGE_SHIFT);
		}
	}
	check(fxfs_sim_size(fs, file_inode(worker->file)->i_ino) == worker->size,
	      "worker %d's file is %llu bytes long, not %lld", worker->id,
	      fxfs_sim_size(fs, file_inode(worker->file)->i_ino), worker->size);
	whole = kmalloc(PRIVATE_MAX, GFP_KERNEL);
	fxfs_sim_read(fs, file_inode(worker->file)->i_ino, 0, whole, worker->size);
	check_bytes("its file's extents", worker, worker->file, whole, worker->private,
		    worker->size, 0);
	kfree(whole);
}

static void setup(void)
{
	static char zeros[2 * PTRS_PER_PTE * PAGE_SIZE];
	ssize_t done;
	int i;

	mount = sim_mount_new();
	shared_file = sim_file_new(mount);
	big_file = sim_file_new(mount);
	done = sim_write(shared_file, 0, zeros, SHARED_PAGES << PAGE_SHIFT);
	check(done == SHARED_PAGES << PAGE_SHIFT, "sizing the shared file: %zd", done);
	/* Nonzero, so that its blocks are written: a sealed extent of a piece, for a PMD. */
	memset(zeros, 0, sizeof(zeros));
	done = sim_write(big_file, 0, zeros, BIG_PAGES << PAGE_SHIFT);
	check(done == BIG_PAGES << PAGE_SHIFT, "sizing the big file: %zd", done);
	for (i = 0; i < SMALL_FILES; i++) {
		small_files[i] = sim_file_new(mount);
		done = sim_write(small_files[i], 0, zeros, SMALL_PAGES << PAGE_SHIFT);
		check(done == SMALL_PAGES << PAGE_SHIFT, "sizing a small file: %zd", done);
		small_addrs[i] = sim_mmap(small_files[i], 0, SMALL_PAGES << PAGE_SHIFT);
	}
	check(!sim_fsync(shared_file), "the first seal");
	shared_addr = sim_mmap(shared_file, 0, SHARED_PAGES << PAGE_SHIFT);
	big_addr = sim_mmap(big_file, 0, BIG_PAGES << PAGE_SHIFT);
	for (i = 0; i < WORKERS; i++) {
		workers[i].id = i;
		workers[i].file = sim_file_new(mount);
		workers[i].private = kzalloc(PRIVATE_MAX, GFP_KERNEL);
		workers[i].addr = sim_mmap(workers[i].file, 0, PRIVATE_MAX);
		workers[i].huge_file = sim_file_new(mount);
		workers[i].huge_addr = sim_mmap(workers[i].huge_file, 0, HUGE_LEN);
	}
	writers_left = WORKERS;
}

/*
 * dax.c's own diagnostics, each of whose WARNs fails the run, as `fxfs.NAME=1` on a command line,
 * but for dax_shadow, which `-s` adds, and whose check of the sealed pages takes it: an evacuation
 * of a page once writable in its range's own space, relocated since, WARNs (README.md).
 */
static void diagnostics(bool shadow)
{
	sim_param_set("dax_check_seals", "1");
	sim_param_set("dax_check_moves", "1");
	sim_param_set("dax_check_places", "1");
	if (shadow) {
		sim_param_set("dax_shadow", "1");
		sim_param_set("dax_verify_seals", "1");
	}
}

/* dax.c's fault injection, as the seed picks: each widens or takes a way the protocol goes. */
static void injections(void)
{
	char value[24];

	/* Its racing steps sleep, which here are scheduling points. */
	snprintf(value, sizeof(value), "%llu", sim_random() % 4 * 10);
	sim_param_set("dax_delay_us", value);
	snprintf(value, sizeof(value), "%llu", sim_random());
	sim_param_set("dax_delay_seed", value);
	if (!(sim_random() % 8)) {
		fault_seals = true;
		sim_param_set("dax_fault_seals", "1");
	}
	if (!(sim_random() % 2))
		sim_param_set("dax_fault_seals_moved", "20");
	if (!(sim_random() % 4))
		sim_param_set("dax_fail_held", "1");
	if (!(sim_random() % 4))
		sim_param_set("dax_fail_evacuees", "1");
	if (!(sim_random() % 4))
		sim_param_set("dax_evacuate_every", "3");
}

int main(int argc, char **argv)
{
	unsigned long long seed = 0;
	bool shadow = false, no_injections = false;
	unsigned long watched_address = 0;
	u64 moved, freed;
	int i;

	if (argc < 2 || sscanf(argv[1], "%llu", &seed) != 1)
		sim_panic("usage: interleave SEED [OPERATIONS] [-s] [-t] [-T] [-v] [-I] [-N] [-w WINDOW_PAGE] [-a ADDRESS] [-W INO PAGE]");
	if (argc > 2 && argv[2][0] != '-')
		sscanf(argv[2], "%lu", &operations);
	sim_init(seed);
	sim_device_init(WINDOW_LEN);
	for (i = 2; i < argc; i++) {
		unsigned long long page;

		if (!strcmp(argv[i], "-t"))
			sim_trace_on();
		else if (!strcmp(argv[i], "-s"))
			shadow = true;
		else if (!strcmp(argv[i], "-T"))
			tracing = true;
		else if (!strcmp(argv[i], "-I"))
			no_injections = true;
		else if (!strcmp(argv[i], "-N"))
			fault_seals = true;
		else if (!strcmp(argv[i], "-v"))
			verbose = true;
		else if (!strcmp(argv[i], "-a"))
			sscanf(argv[i + 1], "%lx", &watched_address);
		else if (!strcmp(argv[i], "-W") && i + 2 < argc) {
			unsigned long long ino, page;

			if (sscanf(argv[i + 1], "%llu", &ino) == 1 &&
			    sscanf(argv[i + 2], "%llu", &page) == 1)
				fxfs_sim_watch(ino, page);
		} else if (!strcmp(argv[i], "-w") && sscanf(argv[i + 1], "%llx", &page) == 1)
			sim_device_watch(page);
	}
	diagnostics(shadow);
	if (!no_injections)
		injections();
	if (tracing)
		sim_param_set("dax_trace", "1");
	setup();
	if (watched_address)
		sim_mm_watch(watched_address);
	sim_spawn("syncer", syncer_task, NULL);
	for (i = 0; i < WORKERS; i++)
		sim_spawn(i == 0 ? "worker0" : i == 1 ? "worker1" : "worker2", worker_task,
			  &workers[i]);
	sim_run();
	/* An append's size goes with the file's own fsync (TASK-92.28). */
	check(!sim_fsync(shared_file), "the last seal");
	check(!sim_fsync(big_file), "the last seal");
	for (i = 0; i < SMALL_FILES; i++)
		check(!sim_fsync(small_files[i]), "the last seal");
	for (i = 0; i < WORKERS; i++)
		check(!sim_fsync(workers[i].file), "the last seal");
	sim_run();
	for (i = 0; i < WORKERS; i++)
		check_extents(&workers[i]);
	fxfs_sim_stats(&moved, &freed);
	sim_log("interleave: seed %llu passed: %llu points, %llu switches, %llu device maps, %llu blocks moved, %llu extents freed, %lu PMDs, trace %llx\n",
		seed, sim_points(), sim_switches(), sim_device_maps(), moved, freed, huge_mapped,
		sim_trace_hash());
	return 0;
}
