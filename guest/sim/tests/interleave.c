// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * Tasks writing, mapping, sealing, truncating and evicting at once, their interleaving the seed's:
 * the same seed plays the same run, whose trace hash says so.
 *
 *   interleave SEED [OPERATIONS] [-t] [-T] [-v] [-I] [-N] [-w WINDOW_PAGE] [-a ADDRESS]
 *              [-W INO PAGE]
 *
 * -t logs each scheduling point, -T has dax.c's trace of the page that fails logged, -w logs the
 * window page's MAPs, -a the address's PTE, and -W what Fxfs does to the file's page; -v each
 * operation. -I leaves dax.c's fault injection off, and -N the operations that take a file's own
 * mapping for their memory. Stores copy on write (doc-13).
 *
 * Every store is stamped, as ztest's blocks carry their tags: whose it is, where it belongs, its
 * generation and the seal it was made in. Tasks share files without sharing bytes: each owns a slot
 * of every page of the shared files, and a private file, cells of stamps. What a task reads of its
 * own must be its last store. What it reads of another's can't be exact, as the owner may be
 * storing, but it is bounded, as ztest's txg bounds a block's: not older than the last store that
 * ended before the read began, nor anything any read saw before it began, nor newer than the last
 * store begun as it ended, and never zeros once written, unless a store was under way. So a stale
 * read, a zero read, a read of another file's or another place's data, or from the future, is named
 * as such as it is read, on any task's read of any slot. After an fsync, Fxfs's extents must hold
 * every slot's last store of the synced file, a store being in a private page until its file's
 * writeback; and once all end, the same of all of it. Each of dax.c's WARNs fails the run.
 */

#include <sim/sim.h>

#define WINDOW_LEN (64UL << 20)
#define SMALL_WINDOW_LEN (16UL << 20)
#define WORKERS 3
#define SHARED_PAGES 64
/* Past a PMD, so that a read fault can map 2 MiB of it at once. */
#define BIG_PAGES (2 * PTRS_PER_PTE)
#define PRIVATE_MAX (32 * PAGE_SIZE)
/* Small enough that they are slices of the staging extent's region, which they share. */
#define SMALL_FILES 4
#define SMALL_PAGES 4
#define HUGE_LEN (2 * PMD_SIZE)
/* How many slots an fsync checks in Fxfs's extents. */
#define FSYNC_CHECKS 16
#define SLOT 32
/* The device's private pages, a run in two: few, so that the budget throttles and rebinds. */
#define COW_PRIVATE_FEW 256

#define check(cond, ...) do { if (!(cond)) sim_panic(__VA_ARGS__); } while (0)

#define STAMP_MAGIC 0xfdaf5eedU

/* What a store leaves in a slot. */
struct stamp {
	u32 magic;
	u32 ino;
	/* Where in the file it belongs, in slots. */
	u32 cell;
	u32 worker;
	/* Of its slot, or for a private file its worker's: from 1. */
	u64 generation;
	/* How many seals had ended as it was made. */
	u32 seals;
	u32 check;
};
_Static_assert(sizeof(struct stamp) == SLOT, "a stamp is a slot");

/* A slot of a page of a file every worker has a slot of each page of. */
struct slot {
	/* The last store to end. */
	struct stamp done;
	/* The generation of the last to begin, the newest a read saw, and if a store is under way. */
	u64 begun, seen;
	bool flying;
};

struct slots {
	struct file *file;
	unsigned long pages;
	/*
	 * By worker, its mapping of the file, as each process sharing a file maps it: a page's read
	 * faults in one mapping must leave the others' alone (TASK-149.14, patch 0010).
	 */
	unsigned long addrs[WORKERS];
	/* By worker, then page. */
	struct slot *slots[WORKERS];
};

struct worker {
	int id;
	/* Its private file, as it should read, cells of stamps, and how many it has made. */
	struct file *file;
	char *private;
	loff_t size;
	u64 generation;
	/* Counts as it begins to shrink and as it ends: loads past the end may fail then. */
	u64 shrinks;
	/* A mapping of its private file, which writes copy from and reads copy to. */
	unsigned long addr;
	/* Held by those using the file, or the mapping, of another's: an inode isn't evicted so. */
	struct rw_semaphore open;
	/* A file that shares blocks of its private file, as it should read, and a mapping of it. */
	struct file *twin;
	char *twin_bytes;
	loff_t twin_size;
	unsigned long twin_addr;
	/* A file of a sealed PMD's data, written again each cycle (huge_cycle). */
	struct file *huge_file;
	unsigned long huge_addr;
};

static struct worker workers[WORKERS];
static struct sim_mount *mount;
static struct slots shared, big, small[SMALL_FILES];
static int writers_left;
static unsigned long operations = 200;
/* Whether the operations that take a file's own mapping for their memory are left out (-N). */
static bool no_self;
/* Whether Fxfs's transactions fail: writes and stores then may too. And whether a MAP does. */
static bool failing, failing_maps;
/*
 * How many pages of a file slots are taken from, in a run that has them all on a few, or 0 for
 * all: most races need two tasks on one page, or one extent.
 */
static unsigned long hot_pages;
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

/* A slot's worth of bytes down from `at`, whatever it is. */
static u64 cell_of(u64 at)
{
	return round_down(at, SLOT);
}

static u32 stamp_hash(const struct stamp *stamp)
{
	const u32 *word = (const u32 *)stamp;
	u32 hash = 0x811c9dc5, i;

	for (i = 0; i < offsetof(struct stamp, check) / sizeof(*word); i++)
		hash = (hash ^ word[i]) * 0x01000193;
	return hash;
}

static void stamp_make(struct stamp *stamp, u64 ino, loff_t at, int worker, u64 generation)
{
	*stamp = (struct stamp) {
		.magic = STAMP_MAGIC, .ino = ino, .cell = at / SLOT, .worker = worker,
		.generation = generation, .seals = fxfs_sim_seals(mount->info->fs),
	};
	stamp->check = stamp_hash(stamp);
}

static bool all_zeros(const void *bytes, size_t len)
{
	const char *byte = bytes;

	return !byte[0] && !memcmp(byte, byte + 1, len - 1);
}

/* A stamp, whatever it is of. */
static bool stamp_valid(const struct stamp *stamp)
{
	return stamp->magic == STAMP_MAGIC && stamp->check == stamp_hash(stamp);
}

static void stamp_log(const char *what, const void *bytes)
{
	struct stamp stamp;

	memcpy(&stamp, bytes, SLOT);
	if (all_zeros(&stamp, SLOT))
		sim_log("interleave: %s: zeros\n", what);
	else if (!stamp_valid(&stamp))
		sim_log("interleave: %s: no stamp\n", what);
	else
		sim_log("interleave: %s: worker %u's generation %llu of ino %u at %llu, made after %u seals, of %llu now\n",
			what, stamp.worker, stamp.generation, stamp.ino, (u64)stamp.cell * SLOT,
			stamp.seals, fxfs_sim_seals(mount->info->fs));
}

/* What is wrong with `got`, read of `ino` at `at`, worker's, between generations: or NULL. */
static const char *stamp_wrong(const struct stamp *got, u64 ino, loff_t at, int worker, u64 low,
			       u64 high, bool overlapped)
{
	if (all_zeros(got, SLOT))
		return low && !overlapped ? "a zero read" : NULL;
	if (!stamp_valid(got))
		return "bytes no store made";
	if (got->ino != ino)
		return "another file's data";
	if (got->cell * SLOT != at)
		return "another place's data";
	if (got->worker != worker)
		return "another worker's data";
	if (got->generation < low)
		return "a stale read";
	if (got->generation > high || got->seals > fxfs_sim_seals(mount->info->fs))
		return "a read from the future";
	return NULL;
}

/* Says all that is known of a wrong read, and ends the run. */
static __noreturn void wrong_read(const char *what, const char *why, struct file *file,
				  const void *got, const void *expected, loff_t at)
{
	u64 ino = file_inode(file)->i_ino;
	char page[48], held[SLOT];

	if (got)
		stamp_log("read", got);
	if (expected)
		stamp_log("expected", expected);
	fxfs_sim_read(mount->info->fs, ino, cell_of(at), held, SLOT);
	stamp_log("in Fxfs's extents", held);
	sim_log("interleave: page %llu of ino %llu\n", at >> PAGE_SHIFT, ino);
	if (tracing) {
		snprintf(page, sizeof(page), "%llu %llu", ino, at >> PAGE_SHIFT);
		sim_param_set("dax_trace_dump", page);
	}
	sim_panic("%s of ino %llu at %lld: %s", what, ino, at, why);
}

/* `got` must be `expected`, byte for byte, of the file's `len` bytes at `at`. */
static void check_bytes(const char *what, struct file *file, const char *got, const char *expected,
			size_t len, loff_t at)
{
	const char *why = "other data than the last store's";
	size_t i, cell;

	for (i = 0; i < len; i++) {
		if (got[i] == expected[i])
			continue;
		if (!got[i])
			why = "a zero read";
		/* The stamp the byte is of, if the read has all of it. */
		cell = cell_of(at + i) - at;
		if (cell_of(at + i) < at || cell + SLOT > len)
			wrong_read(what, why, file, NULL, NULL, at + i);
		wrong_read(what, why, file, got + cell, expected + cell, at + cell);
	}
}

/* Whether an operation that failed by `result` may have, as Fxfs's transactions fail. */
static bool failed_as_asked(ssize_t result)
{
	return failing && (result == -ENOSPC || result == -EFAULT || result == -EIO);
}

/*
 * Whether a read or a load that fell short (`cut`) was made to by fxfs.dax_fail_maps, whose
 * count was `left` before it: it fails any range's map of more than one piece, whichever way in
 * mapped it, a read's too, which says EIO, or stops short, or a load's fault SIGBUS, as of a
 * device's MAP error. Another task's taking one meanwhile passes for this one's.
 */
static bool map_failure_injected(bool cut, unsigned long left)
{
	return cut && sim_param_read("dax_fail_maps") < left;
}

/* sim_read, again as long as fxfs.dax_fail_maps cuts it short: a read never fails else. */
static ssize_t read_file(struct file *file, loff_t at, void *buf, size_t len)
{
	unsigned long left;
	ssize_t done;

	do {
		left = sim_param_read("dax_fail_maps");
		done = sim_read(file, at, buf, len);
	} while (map_failure_injected(done != len, left));
	return done;
}

/* sim_user_load, again as read_file reads. */
static int user_load(unsigned long addr, void *buf, size_t len)
{
	unsigned long left;
	int err;

	do {
		left = sim_param_read("dax_fail_maps");
		err = sim_user_load(addr, buf, len);
	} while (map_failure_injected(err, left));
	return err;
}

static loff_t slot_at(int worker, unsigned long page)
{
	return ((loff_t)page << PAGE_SHIFT) + worker * SLOT;
}

/*
 * A store that failed: its slot may hold what it held, or zeros, as writable DAX's moved blocks
 * held. So the owner reads what it holds now.
 */
static void slot_resync(struct worker *worker, struct slots *file, unsigned long page)
{
	struct slot *slot = &file->slots[worker->id][page];
	loff_t at = slot_at(worker->id, page);
	struct stamp got;
	ssize_t done = read_file(file->file, at, &got, SLOT);

	check(done == SLOT, "reading a slot back: %zd", done);
	if (all_zeros(&got, SLOT)) {
		memset(&slot->done, 0, SLOT);
		slot->seen = 0;
		return;
	}
	check_bytes("a slot after a failed store", file->file, (char *)&got,
		    (char *)&slot->done, SLOT, at);
}

static void slot_store(struct worker *worker, struct slots *file, unsigned long page, bool mapped)
{
	struct slot *slot = &file->slots[worker->id][page];
	loff_t at = slot_at(worker->id, page);
	struct stamp stamp;
	ssize_t done;

	stamp_make(&stamp, file_inode(file->file)->i_ino, at, worker->id, ++slot->begun);
	slot->flying = true;
	done = mapped ? sim_user_store(file->addrs[worker->id] + at, &stamp, SLOT) ?: SLOT :
	       sim_write(file->file, at, &stamp, SLOT);
	check(done == SLOT || failed_as_asked(done), "a store at page %lu of worker %d: %zd", page,
	      worker->id, done);
	if (done == SLOT)
		slot->done = stamp;
	else
		slot_resync(worker, file, page);
	slot->flying = false;
}

/* A read of `owner`'s slot of the page, by read(2) or through the mapping. */
static void slot_read(struct worker *reader, struct slots *file, unsigned long page, int owner,
		      bool mapped)
{
	struct slot *slot = &file->slots[owner][page];
	u64 low = max(slot->done.generation, slot->seen), begun = slot->begun;
	bool flying = slot->flying, overlapped;
	loff_t at = slot_at(owner, page);
	const char *what = mapped ? "a load" : "a read", *wrong;
	struct stamp got;
	ssize_t done;

	done = mapped ? user_load(file->addrs[reader->id] + at, &got, SLOT) ?: SLOT :
	       read_file(file->file, at, &got, SLOT);
	check(done == SLOT, "%s of page %lu of worker %d: %zd", what, page, owner, done);
	overlapped = flying || slot->flying || slot->begun != begun;
	wrong = stamp_wrong(&got, file_inode(file->file)->i_ino, at, owner, low,
			    overlapped ? slot->begun : slot->done.generation, overlapped);
	if (wrong) {
		if (mapped)
			sim_mm_describe(file->addrs[reader->id] + at);
		wrong_read(what, wrong, file->file, &got, overlapped ? NULL : &slot->done, at);
	}
	slot->seen = max(slot->seen, got.generation);
}

static void slot_op(struct worker *worker, struct slots *file)
{
	unsigned long pages = hot_pages ? min(hot_pages, file->pages) : file->pages;
	unsigned long page = between(0, pages - 1);
	u64 how = between(0, 3);

	operation(worker, "%s page %lu of ino %lu\n", how < 2 ? "store" : "read", page,
		  file_inode(file->file)->i_ino);
	if (how < 2)
		slot_store(worker, file, page, how);
	else
		slot_read(worker, file, page, between(0, WORKERS - 1), how == 3);
}

/* Stamps `len` bytes for `file` at `at` from `first` on, cells of a worker's. */
static void new_cells(struct worker *worker, struct file *file, char *to, loff_t at, size_t len)
{
	size_t i;

	for (i = 0; i < len; i += SLOT)
		stamp_make((struct stamp *)(to + i), file_inode(file)->i_ino, at + i, worker->id,
			   ++worker->generation);
}

/* Whether `got` is a cell of the worker's file, but for where: its own writes copy cells around. */
static bool private_cell(const struct worker *worker, const struct stamp *got)
{
	return all_zeros(got, SLOT) ||
	       (stamp_valid(got) && got->ino == file_inode(worker->file)->i_ino &&
		got->worker == worker->id && got->generation <= worker->generation);
}

/*
 * What a write or a zeroing Fxfs failed left: what they reached of their cells, and of the others
 * what they held, or zeros (slot_resync). The file is read whole.
 */
static void private_resync(struct worker *worker)
{
	loff_t size = i_size_read(file_inode(worker->file)), at;
	char *whole = kzalloc(PRIVATE_MAX, GFP_KERNEL), *held;
	ssize_t done = read_file(worker->file, 0, whole, size);

	check(done == size, "reading a file back: %zd of %lld", done, size);
	for (at = 0; at < size; at += SLOT) {
		if (!memcmp(whole + at, worker->private + at, SLOT))
			continue;
		if (!private_cell(worker, (struct stamp *)(whole + at)))
			wrong_read("a file after a failed write", "bytes no store made",
				   worker->file, whole + at, worker->private + at, at);
	}
	/*
	 * Whatever failed, read(2) goes where Fxfs has the file once it is written back, its dirty
	 * pages, what the failure left among them, being in the window alone.
	 */
	check(!sim_fsync(worker->file), "an fsync after a failed write");
	held = kmalloc(PRIVATE_MAX, GFP_KERNEL);
	fxfs_sim_read(mount->info->fs, file_inode(worker->file)->i_ino, 0, held, size);
	check_bytes("a file after a failed write, against Fxfs's extents", worker->file, whole, held,
		    size, 0);
	kfree(held);
	memcpy(worker->private, whole, PRIVATE_MAX);
	worker->size = size;
	kfree(whole);
}

static void private_write(struct worker *worker)
{
	loff_t at = cell_of(between(0, min_t(loff_t, worker->size, PRIVATE_MAX - SLOT)));
	size_t len = cell_of(between(SLOT, min_t(u64, PRIVATE_MAX - at, 3 * PAGE_SIZE)));
	char *buf = kmalloc(len, GFP_KERNEL);
	ssize_t done;

	operation(worker, "write %lld+%zu, size %lld\n", at, len, worker->size);
	new_cells(worker, worker->file, buf, at, len);
	done = sim_write(worker->file, at, buf, len);
	check(done >= 0 ? done == len || failing : failed_as_asked(done),
	      "a private write: %zd of %zu", done, len);
	if (done == len) {
		memcpy(worker->private + at, buf, len);
		worker->size = max_t(loff_t, worker->size, at + len);
	} else {
		private_resync(worker);
	}
	kfree(buf);
}

static void private_read(struct worker *worker)
{
	size_t len;
	loff_t at;
	char *buf;
	ssize_t done;

	if (!worker->size)
		return;
	at = cell_of(between(0, worker->size - 1));
	len = between(1, min_t(u64, worker->size - at, 3 * PAGE_SIZE));
	buf = kmalloc(len, GFP_KERNEL);
	done = read_file(worker->file, at, buf, len);
	check(done == len, "a private read: %zd of %zu", done, len);
	check_bytes("a private read", worker->file, buf, worker->private + at, len, at);
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

	if (worker->size < 4 * PAGE_SIZE || no_self)
		return;
	/* Not in a page of its target, which it unmaps each time: it would fail, as dax.c says. */
	len = cell_of(between(SLOT, PAGE_SIZE));
	from = cell_of(between(0, worker->size - 3 * PAGE_SIZE));
	to = cell_of(between(round_up(from + len, PAGE_SIZE),
			     min_t(loff_t, worker->size, PRIVATE_MAX - len)));
	operation(worker, "write %lld+%zu from the mapping at %lld, size %lld\n", to, len, from,
		  worker->size);
	done = sim_write_user(worker->file, to, worker->addr + from, len);
	check(done >= 0 ? done == len || failing : failed_as_asked(done),
	      "a write from its own mapping: %zd of %zu", done, len);
	if (done == len) {
		memcpy(worker->private + to, worker->private + from, len);
		worker->size = max_t(loff_t, worker->size, to + len);
	} else {
		private_resync(worker);
	}
}

/* A read into a mapping of the file it reads, whose fault may wait for the iomap of the read. */
static void private_self_read(struct worker *worker)
{
	size_t len;
	loff_t from, to;
	ssize_t done;

	if (worker->size < 4 * PAGE_SIZE || no_self)
		return;
	len = cell_of(between(SLOT, PAGE_SIZE));
	from = cell_of(between(0, worker->size - 3 * PAGE_SIZE));
	to = cell_of(between(round_up(from + len, PAGE_SIZE), worker->size - len));
	operation(worker, "read %lld+%zu into the mapping at %lld, size %lld\n", from, len, to,
		  worker->size);
	done = sim_read_user(worker->file, from, worker->addr + to, len);
	check(done >= 0 ? done == len || failing : failed_as_asked(done),
	      "a read into its own mapping: %zd of %zu", done, len);
	if (done == len)
		memcpy(worker->private + to, worker->private + from, len);
	else
		private_resync(worker);
}

/* A load through the mapping must see what read(2) does. */
static void private_load(struct worker *worker)
{
	size_t len, i;
	loff_t at;
	char *buf;
	int err;

	if (!worker->size)
		return;
	at = between(0, worker->size - 1);
	len = between(1, min_t(u64, worker->size - at, 3 * PAGE_SIZE));
	operation(worker, "load %lld+%zu, size %lld\n", at, len, worker->size);
	buf = kmalloc(len, GFP_KERNEL);
	err = user_load(worker->addr + at, buf, len);
	check(!err, "a private load: %d", err);
	for (i = 0; i < len; i++) {
		if (buf[i] != worker->private[at + i]) {
			sim_mm_describe(worker->addr + at + i);
			break;
		}
	}
	check_bytes("a private load", worker->file, buf, worker->private + at, len, at);
	kfree(buf);
}

static void private_truncate(struct worker *worker)
{
	loff_t size = cell_of(between(0, PRIVATE_MAX / 2));
	int err;

	operation(worker, "truncate to %lld, size %lld\n", size, worker->size);
	worker->shrinks++;
	err = sim_truncate(worker->file, size);
	check(!err || failed_as_asked(err), "a truncate: %d", err);
	/*
	 * A growth that failed leaves the file as long as it was: the inode saying a size Fxfs
	 * hasn't, read in again the file is Fxfs's size, the stores past it lost (fxfs_data_resize).
	 */
	check(!err || size <= worker->size || i_size_read(file_inode(worker->file)) == worker->size,
	      "a growth to %lld that failed (%d) left the file %lld bytes long, not %lld", size, err,
	      i_size_read(file_inode(worker->file)), worker->size);
	if (err) {
		private_resync(worker);
	} else {
		if (size < worker->size)
			memset(worker->private + size, 0, worker->size - size);
		worker->size = size;
	}
	worker->shrinks++;
}

static void private_punch(struct worker *worker)
{
	loff_t start, end;
	int err;

	if (worker->size < SLOT)
		return;
	start = cell_of(between(0, worker->size - SLOT));
	end = cell_of(between(start + SLOT, worker->size));
	operation(worker, "punch %lld to %lld, size %lld\n", start, end, worker->size);
	err = sim_punch(worker->file, start, end);
	check(!err || failed_as_asked(err), "a punch: %d", err);
	if (err)
		private_resync(worker);
	else
		memset(worker->private + start, 0, end - start);
}

/*
 * Blocks of the private file shared with its twin, some of them perhaps in an open extent: they
 * read as shared from now on, read-only, and a write moves them (TASK-140, TASK-44.21). What each
 * file holds is its own, whatever is written to the other.
 */
static void private_clone(struct worker *worker)
{
	size_t len;
	loff_t from, to;
	int err;

	if (worker->size < PAGE_SIZE)
		return;
	len = between(1, min_t(u64, 8, worker->size >> PAGE_SHIFT)) << PAGE_SHIFT;
	from = round_down(between(0, worker->size - len), PAGE_SIZE);
	to = round_down(between(0, PRIVATE_MAX - len), PAGE_SIZE);
	operation(worker, "clone %lld+%zu to %lld of the twin, size %lld\n", from, len, to,
		  worker->size);
	err = sim_clone(worker->file, from, worker->twin, to, len);
	check(!err, "a clone: %d", err);
	memcpy(worker->twin_bytes + to, worker->private + from, len);
	worker->twin_size = max_t(loff_t, worker->twin_size, to + len);
}

static void twin_read(struct worker *worker)
{
	char buf[2 * SLOT];
	loff_t at;
	int err;

	if (worker->twin_size < sizeof(buf))
		return;
	at = cell_of(between(0, worker->twin_size - sizeof(buf)));
	operation(worker, "read the twin at %lld, size %lld\n", at, worker->twin_size);
	if (between(0, 1)) {
		err = user_load(worker->twin_addr + at, buf, sizeof(buf));
		check(!err, "a load of the twin: %d", err);
	} else {
		check(read_file(worker->twin, at, buf, sizeof(buf)) == sizeof(buf), "a read of the twin");
	}
	check_bytes("the twin", worker->twin, buf, worker->twin_bytes + at, sizeof(buf), at);
}

/*
 * The inode goes, and another comes, of a file whose ranges stay mapped: what its entries said of
 * its pages goes, and what the ranges say of them stays (dax_folio_put).
 */
static void private_evict(struct worker *worker)
{
	operation(worker, "evict, size %lld\n", worker->size);
	down_write(&worker->open);
	sim_munmap(worker->addr);
	worker->file = sim_file_read_in(mount, worker->file);
	worker->addr = sim_mmap(worker->file, 0, PRIVATE_MAX);
	up_write(&worker->open);
}

/*
 * A load or a read of another's private file, as it grows and shrinks: whatever it finds is the
 * file's, or zeros, and a load fails only past an end that moved. Its fault maps the chunk as large
 * as the file is, racing a write that grows it (TASK-79.28).
 */
static void peek(struct worker *worker)
{
	struct worker *owner = &workers[between(0, WORKERS - 1)];
	char buf[3 * SLOT];
	size_t i, len = sizeof(buf);
	u64 shrinks;
	ssize_t done;
	loff_t at;

	if (owner == worker)
		return;
	down_read(&owner->open);
	shrinks = owner->shrinks;
	if (owner->size < sizeof(buf))
		goto out;
	at = cell_of(between(0, owner->size - sizeof(buf)));
	operation(worker, "peek at worker %d's %lld, size %lld\n", owner->id, at, owner->size);
	if (between(0, 1)) {
		done = user_load(owner->addr + at, buf, len) ?: len;
		check(done == len || failing || shrinks % 2 || owner->shrinks != shrinks,
		      "a peek at worker %d's page %lld: %zd", owner->id, at >> PAGE_SHIFT, done);
		if (done != len)
			goto out;
	} else {
		done = read_file(owner->file, at, buf, len);
		check(done >= 0, "a peek at worker %d's: %zd", owner->id, done);
		len = cell_of(done);
	}
	for (i = 0; i < len; i += SLOT) {
		if (!private_cell(owner, (struct stamp *)(buf + i)))
			wrong_read("a peek", "bytes that aren't the file's", owner->file, buf + i,
				   NULL, at + i);
	}
out:
	up_read(&owner->open);
}

/*
 * What a cycle stores in its file. In one extent a chunk is a slice of its region, which a page
 * moving drops, so that a PMD's fault says shared. In two, each whole, it is a range of its own,
 * its pages in a row, as a PMD's fault finds them; and a page that moves leaves the row (TASK-123),
 * whose PMD entry the faults after replace (patch 0008).
 */
static bool huge_write(struct worker *worker, const char *data, bool pieces)
{
	static const size_t whole[] = { HUGE_LEN }, parts[] = { PMD_SIZE / 2, PMD_SIZE / 2, PMD_SIZE };
	const size_t *lengths = pieces ? parts : whole;
	size_t at = 0, count = pieces ? ARRAY_SIZE(parts) : ARRAY_SIZE(whole), i;
	ssize_t done;
	int err;

	for (i = 0; i < count; at += lengths[i++]) {
		done = sim_write(worker->huge_file, at, data + at, lengths[i]);
		check(done == lengths[i] || failing, "a whole file's write: %zd", done);
		if (done != lengths[i])
			return false;
		err = sim_fsync(worker->huge_file);
		check(!err, "fsync: %d", err);
	}
	return true;
}

/*
 * A chunk mapped by a PMD, whose page a store then makes private, which splits the PMD: its other
 * pages must fault back in, each read what it held (TASK-123). Truncated first, so that no entries
 * of its pages keep a PMD's fault out; and mapped anew, as a PTE table left under the PMD would.
 */
static void huge_cycle(struct worker *worker)
{
	static char data[WORKERS][HUGE_LEN];
	char *mine = data[worker->id], buf[SLOT];
	unsigned long stored = between(0, PTRS_PER_PTE - 1), others[8];
	bool pieces = between(0, 1);
	int err, i, round;

	operation(worker, "a PMD's cycle, %s, storing to page %lu\n",
		  pieces ? "in pieces" : "whole", stored);
	sim_munmap(worker->huge_addr);
	worker->huge_addr = sim_mmap(worker->huge_file, 0, HUGE_LEN);
	err = sim_truncate(worker->huge_file, 0);
	check(!err || failed_as_asked(err), "a truncate: %d", err);
	if (err)
		return;
	new_cells(worker, worker->huge_file, mine, 0, HUGE_LEN);
	if (!huge_write(worker, mine, pieces))
		return;
	err = user_load(worker->huge_addr, buf, SLOT);
	check(!err, "a load of a PMD: %d", err);
	check_bytes("a load of a PMD", worker->huge_file, buf, mine, SLOT, 0);
	huge_mapped += sim_mm_is_huge(worker->huge_addr);
	new_cells(worker, worker->huge_file, buf, stored * PAGE_SIZE, SLOT);
	err = sim_user_store(worker->huge_addr + stored * PAGE_SIZE, buf, SLOT);
	check(!err || failed_as_asked(err), "a store into a PMD: %d", err);
	if (err)
		return;
	memcpy(mine + stored * PAGE_SIZE, buf, SLOT);
	for (i = 0; i < ARRAY_SIZE(others); i++)
		others[i] = between(0, PTRS_PER_PTE - 1);
	/*
	 * Each page faults, and replaces its part of the PMD's entry. Then a mapping new to the
	 * pages, whose faults must find their own entries: as another process's would.
	 */
	for (round = 0; round < 2; round++) {
		for (i = 0; i < ARRAY_SIZE(others); i++) {
			loff_t at = others[i] * PAGE_SIZE;

			err = user_load(worker->huge_addr + at, buf, SLOT);
			check(!err, "a load of page %lu of a broken PMD: %d", others[i], err);
			check_bytes("a load of a broken PMD", worker->huge_file, buf, mine + at,
				    SLOT, at);
		}
		sim_munmap(worker->huge_addr);
		worker->huge_addr = sim_mmap(worker->huge_file, 0, HUGE_LEN);
	}
}

static void worker_task(void *arg)
{
	struct worker *worker = arg;
	unsigned long i;

	for (i = 0; i < operations; i++) {
		switch (between(0, 21)) {
		case 0 ... 3:
			slot_op(worker, &shared);
			break;
		case 13 ... 15:
			slot_op(worker, &small[between(0, SMALL_FILES - 1)]);
			break;
		case 16:
			if (!between(0, 3))
				huge_cycle(worker);
			break;
		case 17 ... 18:
			peek(worker);
			break;
		case 19:
			if (!between(0, 3))
				private_evict(worker);
			break;
		case 20:
			private_clone(worker);
			break;
		case 21:
			twin_read(worker);
			break;
		case 4 ... 5:
			slot_op(worker, &big);
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

/* A slot's last store, whose file hold it must through Fxfs's extents, if none came since. */
struct fsync_check {
	struct slots *file;
	struct slot *slot;
	struct stamp done;
	u64 begun;
	loff_t at;
};

static struct slots *any_slots(void)
{
	u64 which = between(0, SMALL_FILES + 1);

	return which == SMALL_FILES ? &shared : which > SMALL_FILES ? &big : &small[which];
}

static void fsync_watch(struct fsync_check *watched, struct slots *file)
{
	int owner = between(0, WORKERS - 1);

	watched->file = file;
	watched->at = between(0, watched->file->pages - 1);
	watched->slot = &watched->file->slots[owner][watched->at];
	watched->at = slot_at(owner, watched->at);
	watched->done = watched->slot->done;
	watched->begun = watched->slot->begun;
}

/* What the seal made durable, unless a store came since or was under way. */
static void fsync_check(const struct fsync_check *watched)
{
	struct stamp held;

	if (watched->slot->flying || watched->slot->begun != watched->begun ||
	    watched->done.generation != watched->begun)
		return;
	fxfs_sim_read(mount->info->fs, file_inode(watched->file->file)->i_ino, watched->at, &held,
		      SLOT);
	if (memcmp(&held, &watched->done, SLOT))
		wrong_read("Fxfs's extents after an fsync", all_zeros(&held, SLOT) ?
			   "a zero read" : "a stale read", watched->file->file, &held,
			   &watched->done, watched->at);
}

/*
 * Seals as a journal does, whenever it likes: in some runs seldom. A store is in a private page
 * until its file's writeback, and the fsync makes its own file's slots durable, no other's, which
 * are those watched then.
 */
static void syncer_task(void *arg)
{
	struct fsync_check watched[FSYNC_CHECKS];
	u64 pause = between(0, 3) ? 2 : 40;
	struct slots *synced;
	int err, i;

	while (READ_ONCE(writers_left)) {
		synced = any_slots();
		for (i = 0; i < FSYNC_CHECKS; i++)
			fsync_watch(&watched[i], synced);
		err = sim_fsync(synced->file);
		check(!err, "fsync: %d", err);
		for (i = 0; i < FSYNC_CHECKS; i++)
			fsync_check(&watched[i]);
		msleep(between(0, pause));
	}
}

/* Each worker's every slot of a file must be its last store, in Fxfs's extents. */
static void check_slots(const struct slots *file)
{
	struct fxfs *fs = mount->info->fs;
	u64 ino = file_inode(file->file)->i_ino;
	struct stamp held;
	unsigned long page;
	int owner;

	for (owner = 0; owner < WORKERS; owner++) {
		for (page = 0; page < file->pages; page++) {
			fxfs_sim_read(fs, ino, slot_at(owner, page), &held, SLOT);
			check_bytes("a file's extents", file->file, (char *)&held,
				    (char *)&file->slots[owner][page].done, SLOT,
				    slot_at(owner, page));
		}
	}
}

static void check_extents(const struct worker *worker)
{
	struct fxfs *fs = mount->info->fs;
	char *whole;

	check(fxfs_sim_size(fs, file_inode(worker->file)->i_ino) == worker->size,
	      "worker %d's file is %llu bytes long, not %lld", worker->id,
	      fxfs_sim_size(fs, file_inode(worker->file)->i_ino), worker->size);
	whole = kmalloc(PRIVATE_MAX, GFP_KERNEL);
	fxfs_sim_read(fs, file_inode(worker->file)->i_ino, 0, whole, worker->size);
	check_bytes("its file's extents", worker->file, whole, worker->private, worker->size, 0);
	fxfs_sim_read(fs, file_inode(worker->twin)->i_ino, 0, whole, worker->twin_size);
	check_bytes("its twin's extents", worker->twin, whole, worker->twin_bytes,
		    worker->twin_size, 0);
	kfree(whole);
}

static void slots_new(struct slots *file, unsigned long pages)
{
	static char zeros[2 * PTRS_PER_PTE * PAGE_SIZE];
	ssize_t done;
	int i;

	file->file = sim_file_new(mount);
	file->pages = pages;
	for (i = 0; i < WORKERS; i++)
		file->slots[i] = kcalloc(pages, sizeof(*file->slots[i]), GFP_KERNEL);
	done = sim_write(file->file, 0, zeros, pages << PAGE_SHIFT);
	check(done == pages << PAGE_SHIFT, "sizing a file: %zd", done);
}

static void setup(void)
{
	int i;

	mount = sim_mount_new();
	slots_new(&shared, SHARED_PAGES);
	slots_new(&big, BIG_PAGES);
	for (i = 0; i < SMALL_FILES; i++)
		slots_new(&small[i], SMALL_PAGES);
	check(!sim_fsync(shared.file), "the first seal");
	for (i = 0; i < WORKERS; i++) {
		int f;

		for (f = 0; f < SMALL_FILES; f++)
			small[f].addrs[i] = sim_mmap(small[f].file, 0, SMALL_PAGES << PAGE_SHIFT);
		shared.addrs[i] = sim_mmap(shared.file, 0, SHARED_PAGES << PAGE_SHIFT);
		big.addrs[i] = sim_mmap(big.file, 0, BIG_PAGES << PAGE_SHIFT);
	}
	for (i = 0; i < WORKERS; i++) {
		workers[i].id = i;
		workers[i].file = sim_file_new(mount);
		workers[i].private = kzalloc(PRIVATE_MAX, GFP_KERNEL);
		workers[i].addr = sim_mmap(workers[i].file, 0, PRIVATE_MAX);
		init_rwsem(&workers[i].open);
		workers[i].twin = sim_file_new(mount);
		workers[i].twin_bytes = kzalloc(PRIVATE_MAX, GFP_KERNEL);
		workers[i].twin_addr = sim_mmap(workers[i].twin, 0, PRIVATE_MAX);
		workers[i].huge_file = sim_file_new(mount);
		workers[i].huge_addr = sim_mmap(workers[i].huge_file, 0, HUGE_LEN);
	}
	writers_left = WORKERS;
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
	if (!(sim_random() % 4))
		sim_param_set("dax_fail_map_many", "20");
	failing = !(sim_random() % 4);
	/* A range's map of more than one piece fails, as a device's MAP error (TASK-119.13). */
	if (failing && !(sim_random() % 2)) {
		sim_param_set("dax_fail_maps", "20");
		failing_maps = true;
	}
	hot_pages = (const unsigned long[]){ 0, 0, 2, 8 }[sim_random() % 4];
}

int main(int argc, char **argv)
{
	unsigned long long seed = 0;
	bool no_injections = false;
	u64 window_len;
	unsigned long watched_address = 0;
	u64 written, freed, failed;
	int i;

	if (argc < 2 || sscanf(argv[1], "%llu", &seed) != 1)
		sim_panic("usage: interleave SEED [OPERATIONS] [-t] [-T] [-v] [-I] [-N] [-w WINDOW_PAGE] [-a ADDRESS] [-W INO PAGE]");
	if (argc > 2 && argv[2][0] != '-')
		sscanf(argv[2], "%lu", &operations);
	sim_init(seed);
	/* A window its files fill, a run in four, whose ranges are reclaimed and regions run out. */
	window_len = sim_random() % 4 ? WINDOW_LEN : SMALL_WINDOW_LEN;
	sim_device_init(window_len);
	/* The device's private pages, before the mount: few, or the host's default. */
	sim_device_private_max(sim_random() % 2 ? COW_PRIVATE_FEW : window_len / PAGE_SIZE / 4);
	for (i = 2; i < argc; i++) {
		unsigned long long page;

		if (!strcmp(argv[i], "-t"))
			sim_trace_on();
		else if (!strcmp(argv[i], "-T"))
			tracing = true;
		else if (!strcmp(argv[i], "-I"))
			no_injections = true;
		else if (!strcmp(argv[i], "-N"))
			no_self = true;
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
	if (!no_injections)
		injections();
	if (tracing)
		sim_param_set("dax_trace", "1");
	setup();
	if (failing)
		fxfs_sim_fail(32);
	if (verbose)
		sim_log("interleave: %lu hot pages, Fxfs fails %d, a MAP fails %d\n", hot_pages,
			failing, failing_maps);
	if (watched_address)
		sim_mm_watch(watched_address);
	sim_spawn("syncer", syncer_task, NULL);
	for (i = 0; i < WORKERS; i++)
		sim_spawn(i == 0 ? "worker0" : i == 1 ? "worker1" : "worker2", worker_task,
			  &workers[i]);
	sim_run();
	/* An append's size goes with the file's own fsync (TASK-92.28). */
	check(!sim_fsync(shared.file), "the last seal");
	check(!sim_fsync(big.file), "the last seal");
	for (i = 0; i < SMALL_FILES; i++)
		check(!sim_fsync(small[i].file), "the last seal");
	for (i = 0; i < WORKERS; i++)
		check(!sim_fsync(workers[i].file), "the last seal");
	sim_run();
	check_slots(&shared);
	check_slots(&big);
	for (i = 0; i < SMALL_FILES; i++)
		check_slots(&small[i]);
	for (i = 0; i < WORKERS; i++)
		check_extents(&workers[i]);
	sim_end(SIM_END_FILES_READ);
	fxfs_sim_stats(&written, &freed, &failed);
	sim_log("interleave: seed %llu passed: %llu points, %llu switches, %llu device maps, %llu blocks written back, %llu extents freed, %llu transactions failed, %lu PMDs, trace %llx\n",
		seed, sim_points(), sim_switches(), sim_device_maps(), written, freed, failed,
		huge_mapped, sim_trace_hash());
	return 0;
}
