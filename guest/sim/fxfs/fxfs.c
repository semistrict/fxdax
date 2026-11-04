// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * A model of Fxfs, as much of fs/fxfs/fxfs.h as dax.c calls: files as arrays of blocks, in extents
 * of the store, with the rules of guest/rust/fxfs-linux and crates/fxfs that the window's protocol
 * rests on (txn_map_write, the allocator's staging extent, Seal):
 *
 * - Blocks of open extents stay where they are; others move, to the staging extent if they are at
 *   most STAGED_MAX bytes, which files share, or an extent of their own. A move keeps what the
 *   range's first and last blocks hold around it, or all it holds (`keep`), or `held`.
 * - An extent is open as it is minted, until a seal, for good: minted, open, sealed, and never back
 *   (OpenExtents). A seal forgets which are open as it begins and as it ends, and the device seals
 *   those it holds. One no request reached, the device takes a write to, which leaves it not open,
 *   until the next seal seals it. A transaction whose device write finds its extent sealed is
 *   dropped, and tried again.
 * - A cut that isn't told what the kernel keeps of the last block (FXFS_SET_KEEP_TAIL) zeros what
 *   that block holds past the end by moving it (zero_tail), and so does a grow, past the old end.
 * - An extent dies once nothing holds a block of it and it isn't the staging extent, and is freed
 *   after a device flush that follows its death (FreeAfter::Flush): one seal in SIM_FLUSH_SEALS here.
 *   Until then the window goes on showing it; then the device maps zeros.
 *
 * - Blocks a clone shares take no write in place: they read as sealed, whatever their extent is, and
 *   a write moves them, though the extent's other blocks may be open (TASK-140, TASK-44.21).
 * - A transaction fails, if a test asks (fxfs_sim_fail), before it does anything, as one out of space
 *   does, or a retry's: what's left of a move a fault went round for is dax.c's to mend (TASK-133).
 *   A write(2)'s fails as it commits, too, after its bytes (fxfs_dax_end_write).
 * - A transaction holds its file's lock, a write(2)'s from its move to its commit (TASK-153.5): until
 *   then the file has its old blocks, and the new ones are held, not the file's.
 * - Writeback (fxfs_write_many), the page cache's and copy-on-write DAX's (doc-13), allocates at
 *   writeback: it copies its pages, a page at a time, into its own buffer, writes them to a new
 *   extent, in one device write no seal comes into, and commits after the bytes: a crash before
 *   the commit leaves the old blocks, which a test may look at, holding the commit
 *   (fxfs_sim_hold_commits), or have fail (fxfs_set_fail_commits). It takes the room the dirty
 *   pages reserved (fxfs_reserve_dirty), done or failed, which nothing may give back twice.
 *
 * It has no journal, no tree, no compaction, no crash consistency: durability isn't simulated, and
 * the real Fxfs's own bugs can't show up here. It's what the window's protocol assumes of Fxfs,
 * written a second time; where it differs from Fxfs, a run proves nothing.
 */

#include <sim/sim.h>
#include <sim/host.h>

#include "../../linux/fs/fxfs/device.h"
#include "../../linux/fs/fxfs/fxfs.h"

#define STAGING_PAGES ((2UL << 20) >> PAGE_SHIFT)
#define STAGED_MAX_PAGES ((256UL << 10) >> PAGE_SHIFT)
/* A seal in so many flushes the device, which frees what died before it. */
#define SIM_FLUSH_SEALS 4
/* How often a transaction that met a seal is tried again: MAX_SEALED_RETRIES's sense. */
#define SEALED_RETRIES 1000

struct stub_extent {
	u64 sequence;
	u64 pages;
	/* Blocks handed out, as far as a staging extent reaches: all of any other. */
	u64 used;
	/* Whether Fxfs knows it to be open: true as it is minted, then false for good. */
	bool open;
	/* Blocks files hold of it, and how many hold each. */
	u64 live;
	u16 *refs;
	/* How many seals had ended when it lost its last block. */
	u64 died;
	struct list_head list;
};

/* A block of a file. */
struct block {
	/* 0: a hole. */
	u64 sequence;
	u64 page;
};

struct stub_file {
	/* Held by each transaction of the file, which takes it before the filesystem's. */
	struct mutex lock;
	u64 ino;
	u64 size;
	u64 nr_blocks;
	struct block *blocks;
	/* The bytes of its dirty pages it holds room for (fxfs_reserve_dirty). */
	u64 reserved;
	struct list_head list;
};

struct fxfs {
	struct fxfs_device *dev;
	/* Held by each transaction as it changes things: the journal's lock, the files' after it. */
	struct mutex lock;
	/* One seal at a time. */
	struct mutex seal;
	/* The device writes in flight, shared, which a seal closes (Seal::begin's `writes`). */
	struct rw_semaphore writes;
	struct list_head extents, files;
	struct stub_extent *staging;
	u64 staging_next;
	u64 next_sequence, next_ino;
	u64 branch;
	u64 seals;
};

static u64 moves, frees, failures;
/* One transaction in so many fails, if any (fxfs_sim_fail). */
static unsigned int fail_odds;
/* How many of the next commits of write(2)s' moves fail (fxfs_sim_fail_commits). */
static unsigned int commits_to_fail;
/* The file and page whose blocks the log tells of, if any (fxfs_sim_watch). */
static u64 watched_ino, watched_page = ~0ULL;

void fxfs_sim_watch(u64 ino, u64 page)
{
	watched_ino = ino;
	watched_page = page;
}

void sim_device_wrote(u64 sequence, u64 page, u64 pages);
bool sim_device_sealed(u64 sequence);
u64 sim_device_length(u64 sequence);

void fxfs_sim_stats(u64 *moved, u64 *freed, u64 *failed)
{
	*moved = moves;
	*freed = frees;
	*failed = failures;
}

void fxfs_sim_fail(unsigned int odds)
{
	fail_odds = odds;
}

void fxfs_sim_fail_commits(unsigned int count)
{
	commits_to_fail = count;
}

u64 fxfs_sim_seals(struct fxfs *fs)
{
	return fs->seals;
}

bool fxfs_sim_sealing(struct fxfs *fs)
{
	/* The device counts a seal's beginning and its end. */
	return fxfs_device_seals(fs->dev) % 2;
}

struct fxfs *fxfs_sim_new(void)
{
	struct fxfs *fs = kzalloc(sizeof(*fs), GFP_KERNEL);
	struct fxfs_device_info info;

	fs->dev = fxfs_device_find("sim", 3, &info);
	if (!fs->dev)
		sim_panic("no device");
	mutex_init(&fs->lock);
	mutex_init(&fs->seal);
	init_rwsem(&fs->writes);
	INIT_LIST_HEAD(&fs->extents);
	INIT_LIST_HEAD(&fs->files);
	fs->next_sequence = 1;
	fs->next_ino = 100;
	fs->branch = info.branch;
	return fs;
}

void fxfs_sim_free(struct fxfs *fs)
{
	struct stub_extent *extent, *next_extent;
	struct stub_file *file, *next_file;

	list_for_each_entry_safe(file, next_file, &fs->files, list) {
		list_del(&file->list);
		kfree(file->blocks);
		kfree(file);
	}
	list_for_each_entry_safe(extent, next_extent, &fs->extents, list) {
		list_del(&extent->list);
		kfree(extent->refs);
		kfree(extent);
	}
	fxfs_device_put(fs->dev);
	kfree(fs);
}

struct fxfs_device *fxfs_dax_device(struct fxfs_filesystem *filesystem)
{
	return ((struct fxfs *)filesystem)->dev;
}

void fxfs_set_check_kept(bool on)
{
}

u64 fxfs_sim_new_file(struct fxfs *fs)
{
	struct stub_file *file = kzalloc(sizeof(*file), GFP_KERNEL);

	mutex_lock(&fs->lock);
	mutex_init(&file->lock);
	file->ino = fs->next_ino++;
	list_add(&file->list, &fs->files);
	mutex_unlock(&fs->lock);
	return file->ino;
}

static struct stub_file *find_file(struct fxfs *fs, u64 ino)
{
	struct stub_file *file;

	list_for_each_entry(file, &fs->files, list) {
		if (file->ino == ino)
			return file;
	}
	sim_panic("no file %llu", ino);
}

static struct stub_extent *find_extent(struct fxfs *fs, u64 sequence)
{
	struct stub_extent *extent;

	list_for_each_entry(extent, &fs->extents, list) {
		if (extent->sequence == sequence)
			return extent;
	}
	sim_panic("no extent %llu", sequence);
}

/* The file `ino`, its lock held, as a transaction of it holds it. */
static struct stub_file *lock_file(struct fxfs *fs, u64 ino)
{
	struct stub_file *file;

	mutex_lock(&fs->lock);
	file = find_file(fs, ino);
	mutex_unlock(&fs->lock);
	mutex_lock(&file->lock);
	return file;
}

/* Room for `pages` blocks of `file`. */
static void reserve_blocks(struct stub_file *file, u64 pages)
{
	if (pages <= file->nr_blocks)
		return;
	file->blocks = krealloc(file->blocks, pages * sizeof(*file->blocks), GFP_NOFS);
	memset(&file->blocks[file->nr_blocks], 0, (pages - file->nr_blocks) * sizeof(*file->blocks));
	file->nr_blocks = pages;
}

static struct stub_extent *new_extent(struct fxfs *fs, u64 pages)
{
	struct stub_extent *extent = kzalloc(sizeof(*extent), GFP_NOFS);

	extent->sequence = fs->next_sequence++;
	extent->pages = pages;
	extent->used = pages;
	extent->refs = kcalloc(pages, sizeof(*extent->refs), GFP_NOFS);
	/* Opened as it is minted, even during a seal, which forgets it as it ends. */
	extent->open = true;
	list_add(&extent->list, &fs->extents);
	sim_device_new_extent(extent->sequence, pages << PAGE_SHIFT);
	return extent;
}

/* `pages` pages of the staging extent, a new one if it's sealed or too full: the first page. */
static u64 take_staged(struct fxfs *fs, u64 pages, struct stub_extent **extent)
{
	u64 page;

	if (!fs->staging || !fs->staging->open || fs->staging_next + pages > fs->staging->pages) {
		fs->staging = new_extent(fs, STAGING_PAGES);
		fs->staging->used = 0;
		fs->staging_next = 0;
	}
	page = fs->staging_next;
	fs->staging_next += pages;
	fs->staging->used = fs->staging_next;
	*extent = fs->staging;
	return page;
}

/* How far `extent` reaches, as allocated, or for the staging extent, while it is open, may yet. */
static u64 extent_bytes(const struct fxfs *fs, const struct stub_extent *extent)
{
	return (extent == fs->staging && extent->open ? extent->pages : extent->used) << PAGE_SHIFT;
}

static void put_block(struct fxfs *fs, struct block *block)
{
	if (block->sequence) {
		struct stub_extent *extent = find_extent(fs, block->sequence);

		extent->live--;
		extent->refs[block->page]--;
		extent->died = fs->seals;
	}
	block->sequence = 0;
}

static void read_block(struct fxfs *fs, const struct block *block, void *buf)
{
	/* Past what the device holds of it, a block reads as zeros. */
	if (block->sequence && block->page < sim_device_length(block->sequence))
		host_store_read(sim_device_store_offset(block->sequence) +
				(block->page << PAGE_SHIFT), buf, PAGE_SIZE);
	else
		memset(buf, 0, PAGE_SIZE);
}

void fxfs_sim_read(struct fxfs *fs, u64 ino, u64 offset, void *buf, size_t len)
{
	struct stub_file *file = find_file(fs, ino);
	char page[PAGE_SIZE];
	size_t done = 0;

	while (done < len) {
		u64 at = (offset + done) >> PAGE_SHIFT;
		size_t in_page = offset_in_page(offset + done);
		size_t part = min_t(size_t, PAGE_SIZE - in_page, len - done);

		if (at < file->nr_blocks)
			read_block(fs, &file->blocks[at], page);
		else
			memset(page, 0, PAGE_SIZE);
		memcpy(buf + done, page + in_page, part);
		done += part;
	}
}

u64 fxfs_sim_size(struct fxfs *fs, u64 ino)
{
	return find_file(fs, ino)->size;
}

ssize_t fxfs_dax_extents(struct fxfs *fs, u64 ino, u64 offset, u64 len, struct fxfs_dax_extent *out,
			 size_t max)
{
	struct stub_file *file;
	u64 page = offset >> PAGE_SHIFT, end = (offset + len) >> PAGE_SHIFT;
	ssize_t count = 0;
	u32 access;

	mutex_lock(&fs->lock);
	file = find_file(fs, ino);
	end = min(end, file->nr_blocks);
	for (; page < end; page++) {
		const struct block *block = &file->blocks[page];
		struct stub_extent *extent;

		if (!block->sequence)
			continue;
		extent = find_extent(fs, block->sequence);
		access = !extent->open ? FXFS_DAX_SEALED :
			 extent->refs[block->page] > 1 ? FXFS_DAX_SHARED : FXFS_DAX_OPEN;
		if (count && out[count - 1].sequence == block->sequence &&
		    out[count - 1].access == access &&
		    out[count - 1].offset + out[count - 1].len == page << PAGE_SHIFT &&
		    out[count - 1].extent_offset + out[count - 1].len ==
		    block->page << PAGE_SHIFT) {
			out[count - 1].len += PAGE_SIZE;
			continue;
		}
		if (count == max) {
			count = -E2BIG;
			break;
		}
		out[count++] = (struct fxfs_dax_extent) {
			.offset = page << PAGE_SHIFT, .len = PAGE_SIZE, .branch = fs->branch,
			.sequence = block->sequence, .extent_offset = block->page << PAGE_SHIFT,
			.access = access, .extent_size = extent_bytes(fs, extent),
		};
	}
	mutex_unlock(&fs->lock);
	return count;
}

int fxfs_dax_update_size(struct fxfs *fs, u64 ino, u64 size)
{
	struct stub_file *file = lock_file(fs, ino);

	mutex_lock(&fs->lock);
	file->size = max(file->size, size);
	mutex_unlock(&fs->lock);
	mutex_unlock(&file->lock);
	return 0;
}

/* Whether a block is anything but zeros, which new blocks hold already. */
static bool keeps_data(const void *data)
{
	const u64 *word = data;
	size_t i;

	for (i = 0; i < PAGE_SIZE / sizeof(*word); i++) {
		if (word[i])
			return true;
	}
	return false;
}

/*
 * A move's new blocks, of the `pages` pages of `file` from `first` that move (`moving`), which a
 * commit puts in place of the old: a write(2)'s, made and not committed (fxfs_dax_begin_write),
 * holding the file's lock until it is.
 */
struct fxfs_dax_write {
	struct fxfs *fs;
	struct stub_file *file;
	u64 first, pages, size;
	struct block *moved;
	bool *moving;
	/* What each page's new block holds of the old, for the log: nothing, what's held, or read. */
	const char **keeping;
};

static void free_move(struct fxfs_dax_write *move)
{
	kfree(move->moved);
	kfree(move->moving);
	kfree(move->keeping);
	kfree(move);
}

/* Gives back the new blocks of `move`, uncommitted. With the filesystem's lock held. */
static void release_move(struct fxfs_dax_write *move)
{
	struct stub_extent *extent;
	u64 page;

	for (page = 0; page < move->pages; page++) {
		if (!move->moving[page])
			continue;
		extent = find_extent(move->fs, move->moved[page].sequence);
		extent->live--;
		extent->refs[move->moved[page].page]--;
	}
}

/* The commit of `move`: the new blocks in place of the old, and the size. With both locks held. */
static void commit_move(struct fxfs_dax_write *move)
{
	struct stub_file *file = move->file;
	u64 page;

	for (page = 0; page < move->pages; page++) {
		struct block *block = &file->blocks[move->first + page];

		if (file->ino == watched_ino && move->first + page == watched_page)
			sim_log("sim: ino %llu page %llu: map_write by %s commits, %s from extent %llu page %llu to extent %llu page %llu, keeping %s, at %llu ns\n",
				file->ino, watched_page, current->comm,
				move->moving[page] ? "moved" : "stays", block->sequence, block->page,
				move->moved[page].sequence, move->moved[page].page,
				move->keeping[page], ktime_get_ns());
		if (move->moving[page]) {
			put_block(move->fs, block);
			*block = move->moved[page];
		}
	}
	file->size = max(file->size, move->size);
}

/*
 * The pieces of `move`'s pages, as they are once it commits, into `out`: how many. With the
 * filesystem's lock held.
 */
static ssize_t move_pieces(struct fxfs_dax_write *move, struct fxfs_dax_extent *out, size_t max)
{
	struct fxfs *fs = move->fs;
	ssize_t count = 0;
	u64 page;

	for (page = 0; page < move->pages; page++) {
		const struct block *block = move->moving[page] ? &move->moved[page] :
					    &move->file->blocks[move->first + page];
		struct stub_extent *extent;

		if (count && out[count - 1].sequence == block->sequence &&
		    out[count - 1].extent_offset + out[count - 1].len ==
		    block->page << PAGE_SHIFT) {
			out[count - 1].len += PAGE_SIZE;
			continue;
		}
		if (count == max) {
			sim_panic("map_write: room for %zu pieces", max);
		}
		extent = find_extent(fs, block->sequence);
		out[count++] = (struct fxfs_dax_extent) {
			.offset = (move->first + page) << PAGE_SHIFT, .len = PAGE_SIZE,
			.branch = fs->branch, .sequence = block->sequence,
			.extent_offset = block->page << PAGE_SHIFT,
			.access = extent->open ? FXFS_DAX_OPEN : FXFS_DAX_SEALED,
			.extent_size = extent->pages << PAGE_SHIFT,
		};
	}
	return count;
}

/*
 * One try at fxfs_dax_map_write's transaction, or fxfs_dax_begin_write's if `pending`, which is
 * left holding the move, and the file's lock: -EAGAIN if a seal came.
 */
static ssize_t try_map_write(struct fxfs *fs, u64 ino, u64 offset, u64 len, u64 size, bool keep,
			     const void *held, struct fxfs_dax_extent *out, size_t max,
			     struct fxfs_dax_write **pending)
{
	u64 first = offset >> PAGE_SHIFT, last = (offset + len - 1) >> PAGE_SHIFT, page, pages;
	struct fxfs_dax_write *move;
	struct block *moved;
	char *copies;
	bool *kept, *moving;
	struct stub_file *file;
	struct stub_extent *extent = NULL;
	ssize_t count = 0;
	int err = 0;

	pages = last - first + 1;
	file = lock_file(fs, ino);
	mutex_lock(&fs->lock);
	reserve_blocks(file, last + 1);
	moved = kcalloc(pages, sizeof(*moved), GFP_NOFS);
	copies = kmalloc(pages << PAGE_SHIFT, GFP_NOFS);
	kept = kcalloc(pages, sizeof(*kept), GFP_NOFS);
	moving = kcalloc(pages, sizeof(*moving), GFP_NOFS);
	move = kzalloc(sizeof(*move), GFP_NOFS);
	*move = (struct fxfs_dax_write) {
		.fs = fs, .file = file, .first = first, .pages = pages, .size = size,
		.moved = moved, .moving = moving,
		.keeping = kcalloc(pages, sizeof(*move->keeping), GFP_NOFS),
	};
	for (page = 0; page < pages; page++) {
		const struct block *block = &file->blocks[first + page];
		bool all = keep || held;

		moving[page] = !block->sequence || !find_extent(fs, block->sequence)->open ||
			       find_extent(fs, block->sequence)->refs[block->page] > 1;
		move->keeping[page] = "nothing";
		if (!moving[page])
			continue;
		moves++;
		/* The first and last blocks hold what the range doesn't, around it. */
		kept[page] = all || (!page && offset_in_page(offset)) ||
			     (page == pages - 1 && offset_in_page(offset + len));
		move->keeping[page] = !kept[page] ? "nothing" : held ? "what's held" : "what it read";
		if (held && kept[page])
			memcpy(copies + (page << PAGE_SHIFT), held + (page << PAGE_SHIFT),
			       PAGE_SIZE);
		else if (kept[page]) {
			/* What a block holds past the file's end isn't the file's (read_aligned). */
			u64 at = (first + page) << PAGE_SHIFT;
			u64 in_file = file->size > at ? min(file->size - at, PAGE_SIZE) : 0;

			read_block(fs, block, copies + (page << PAGE_SHIFT));
			memset(copies + (page << PAGE_SHIFT) + in_file, 0, PAGE_SIZE - in_file);
		}
	}
	/* The parts that move: their new blocks, and their copies to the device. */
	for (page = 0; page < pages;) {
		u64 run = 0, at;

		if (!moving[page]) {
			page++;
			continue;
		}
		while (page + run < pages && moving[page + run])
			run++;
		for (at = 0; at < run;) {
			u64 take = run - at, in_extent;

			if (take <= STAGED_MAX_PAGES) {
				in_extent = take_staged(fs, take, &extent);
			} else {
				extent = new_extent(fs, take);
				in_extent = 0;
			}
			for (; take; take--, at++, in_extent++) {
				moved[page + at] = (struct block) { extent->sequence, in_extent };
				extent->live++;
				extent->refs[in_extent]++;
			}
		}
		page += run;
	}
	/* The device's writes: a seal may come between them, which sealed what they are of. */
	mutex_unlock(&fs->lock);
	/* A transaction takes as long as the journal does: a seal fits in it, a device write late. */
	if (!(sim_random() % 4))
		msleep(1);
	for (page = 0; page < pages; page++) {
		const void *data = copies + (page << PAGE_SHIFT);

		if (kept[page] && moving[page] && !keeps_data(data))
			move->keeping[page] = held ? "what's held, zeros" : "what it read, zeros";
		if (!kept[page] || !moving[page] || !keeps_data(data))
			continue;
		sim_point("a Fxfs write");
		down_read(&fs->writes);
		err = sim_device_sealed(moved[page].sequence) ? -EAGAIN : 0;
		if (!err) {
			host_store_write(sim_device_store_offset(moved[page].sequence) +
					 (moved[page].page << PAGE_SHIFT), data, PAGE_SIZE);
			sim_device_wrote(moved[page].sequence, moved[page].page, 1);
			sim_point("a Fxfs write's reply");
		}
		up_read(&fs->writes);
		if (err)
			break;
	}
	kfree(copies);
	kfree(kept);
	mutex_lock(&fs->lock);
	if (err) {
		release_move(move);
		mutex_unlock(&fs->lock);
		mutex_unlock(&file->lock);
		free_move(move);
		return err;
	}
	count = move_pieces(move, out, max);
	if (pending) {
		mutex_unlock(&fs->lock);
		*pending = move;
		return count;
	}
	commit_move(move);
	mutex_unlock(&fs->lock);
	mutex_unlock(&file->lock);
	free_move(move);
	return count;
}

/* fxfs_dax_map_write, or fxfs_dax_begin_write if `pending`. */
static ssize_t map_write(struct fxfs *fs, u64 ino, u64 offset, u64 len, u64 size, bool keep,
			 const void *held, struct fxfs_dax_extent *out, size_t max,
			 struct fxfs_dax_write **pending)
{
	ssize_t count = -EAGAIN;
	int tries;

	if (fail_odds && !(sim_random() % fail_odds)) {
		failures++;
		return -ENOSPC;
	}
	for (tries = 0; tries < SEALED_RETRIES && count == -EAGAIN; tries++)
		count = try_map_write(fs, ino, offset, len, size, keep, held, out, max, pending);
	if (count == -EAGAIN)
		sim_panic("map_write: its extents sealed still, tried at length");
	return count;
}

ssize_t fxfs_dax_map_write(struct fxfs *fs, u64 ino, u64 offset, u64 len, u64 size, bool keep,
			   const void *held, struct fxfs_dax_extent *out, size_t max)
{
	return map_write(fs, ino, offset, len, size, keep, held, out, max, NULL);
}

ssize_t fxfs_dax_begin_write(struct fxfs *fs, u64 ino, u64 offset, u64 len, u64 size,
			     struct fxfs_dax_extent *out, size_t max, struct fxfs_dax_write **write)
{
	return map_write(fs, ino, offset, len, size, false, NULL, out, max, write);
}

int fxfs_dax_end_write(struct fxfs_dax_write *write, bool commit)
{
	struct fxfs *fs = write->fs;
	int err = 0;

	/* A commit takes the journal's time: a seal fits in it. */
	if (commit && !(sim_random() % 4))
		msleep(1);
	if (commit && (commits_to_fail || (fail_odds && !(sim_random() % fail_odds)))) {
		commits_to_fail -= !!commits_to_fail;
		failures++;
		err = -EIO;
	}
	sim_point(commit && !err ? "a write's commit" : "a write's drop");
	mutex_lock(&fs->lock);
	if (commit && !err)
		commit_move(write);
	else
		release_move(write);
	mutex_unlock(&fs->lock);
	mutex_unlock(&write->file->lock);
	free_move(write);
	return err;
}

bool fxfs_dax_restage(struct fxfs_filesystem *filesystem, struct fxfs_dax_extent *out)
{
	struct fxfs *fs = (struct fxfs *)filesystem;
	bool restaged = false;

	mutex_lock(&fs->lock);
	if (fs->staging && fs->staging_next && !fs->staging->open) {
		fs->staging = new_extent(fs, STAGING_PAGES);
		fs->staging->used = 0;
		fs->staging_next = 0;
		*out = (struct fxfs_dax_extent) {
			.branch = fs->branch, .sequence = fs->staging->sequence,
			.access = FXFS_DAX_OPEN, .extent_size = STAGING_PAGES << PAGE_SHIFT,
		};
		restaged = true;
	}
	mutex_unlock(&fs->lock);
	return restaged;
}

/* Whether seals wait before their COMMIT (fxfs_sim_hold_seals), and whether one does. */
static bool hold_seals, seal_held;

void fxfs_sim_hold_seals(bool hold)
{
	hold_seals = hold;
}

bool fxfs_sim_seal_held(void)
{
	return seal_held;
}

/*
 * A seal, around the COMMIT that seals what is open, and then what it left dead is freed. As the
 * guest's Fxfs's (fxfs-linux's Seal::begin_with), what was minted before it is no longer open from
 * its beginning, before the COMMIT has sealed it on the device.
 */
int fxfs_fsync(struct fxfs *fs)
{
	struct stub_extent *extent, *next;

	mutex_lock(&fs->seal);
	down_write(&fs->writes);
	fxfs_device_seal_begin(fs->dev);
	mutex_lock(&fs->lock);
	list_for_each_entry(extent, &fs->extents, list)
		extent->open = false;
	mutex_unlock(&fs->lock);
	/* A commit takes the device's time: long enough, at times, for transactions to fall in it. */
	if (!(sim_random() % 4))
		msleep(1);
	seal_held = hold_seals;
	while (hold_seals)
		msleep(1);
	seal_held = false;
	sim_point("a seal's COMMIT");
	/*
	 * Those minted meanwhile are forgotten as the COMMIT seals what of them was written, in one
	 * step: between the two, a truncate's move_tail, which takes no `writes`, found the staging
	 * extent open still, and wrote it, sealed (interleave -C, seed 32826).
	 */
	mutex_lock(&fs->lock);
	sim_device_seal();
	list_for_each_entry(extent, &fs->extents, list)
		extent->open = false;
	mutex_unlock(&fs->lock);
	sim_point("a seal's end");
	fxfs_device_seal_end(fs->dev);
	up_write(&fs->writes);
	mutex_lock(&fs->lock);
	fs->seals++;
	list_for_each_entry_safe(extent, next, &fs->extents, list) {
		if (fs->seals % SIM_FLUSH_SEALS || extent->live || extent == fs->staging ||
		    extent->died >= fs->seals - 1)
			continue;
		list_del(&extent->list);
		sim_device_free_extent(extent->sequence);
		frees++;
		kfree(extent);
	}
	mutex_unlock(&fs->lock);
	mutex_unlock(&fs->seal);
	return 0;
}

/*
 * zero_tail, a file `size` bytes long's, as its last block's multi_write has it: the block moves, to
 * a block holding what it held up to `size`, written through the device. A seal between would
 * have it tried again: the model takes no scheduling point in it.
 */
static void move_tail(struct fxfs *fs, struct stub_file *file, u64 size)
{
	u64 at = size >> PAGE_SHIFT, in_extent;
	struct stub_extent *extent;
	char data[PAGE_SIZE];

	if (!offset_in_page(size) || at >= file->nr_blocks || !file->blocks[at].sequence)
		return;
	read_block(fs, &file->blocks[at], data);
	memset(data + offset_in_page(size), 0, PAGE_SIZE - offset_in_page(size));
	in_extent = take_staged(fs, 1, &extent);
	put_block(fs, &file->blocks[at]);
	file->blocks[at] = (struct block) { extent->sequence, in_extent };
	extent->live++;
	extent->refs[in_extent]++;
	host_store_write(sim_device_store_offset(extent->sequence) + (in_extent << PAGE_SHIFT), data,
			 PAGE_SIZE);
	sim_device_wrote(extent->sequence, in_extent, 1);
}

int fxfs_setattr(struct fxfs *fs, u64 ino, const struct fxfs_setattr *set)
{
	struct stub_file *file = lock_file(fs, ino);
	u64 page, old;

	mutex_lock(&fs->lock);
	if (set->valid & FXFS_SET_SIZE) {
		old = file->size;
		for (page = DIV_ROUND_UP(set->size, PAGE_SIZE); page < file->nr_blocks; page++)
			put_block(fs, &file->blocks[page]);
		if (set->size > old)
			move_tail(fs, file, old);
		else if (set->size < old && !(set->valid & FXFS_SET_KEEP_TAIL))
			move_tail(fs, file, set->size);
		if (ino == watched_ino)
			sim_log("sim: ino %llu size %llu -> %llu, by %s, at %llu ns\n", ino, file->size,
				set->size, current->comm, ktime_get_ns());
		file->size = set->size;
	}
	mutex_unlock(&fs->lock);
	mutex_unlock(&file->lock);
	return 0;
}

int fxfs_clone_range(struct fxfs *fs, u64 src, u64 offset, u64 len, u64 dst, u64 dst_offset,
		     u64 size, struct fxfs_attr *attr)
{
	struct stub_file *from, *to;
	u64 page, pages = len >> PAGE_SHIFT;
	struct block *block;

	/* By number, as Fxfs takes a transaction's locks in order. */
	from = lock_file(fs, min(src, dst));
	to = src == dst ? from : lock_file(fs, max(src, dst));
	if (src > dst)
		swap(from, to);
	mutex_lock(&fs->lock);
	reserve_blocks(from, (offset >> PAGE_SHIFT) + pages);
	reserve_blocks(to, (dst_offset >> PAGE_SHIFT) + pages);
	for (page = 0; page < pages; page++) {
		block = &to->blocks[(dst_offset >> PAGE_SHIFT) + page];
		put_block(fs, block);
		*block = from->blocks[(offset >> PAGE_SHIFT) + page];
		if (block->sequence) {
			struct stub_extent *extent = find_extent(fs, block->sequence);

			extent->live++;
			extent->refs[block->page]++;
		}
	}
	to->size = max(to->size, size);
	mutex_unlock(&fs->lock);
	if (to != from)
		mutex_unlock(&to->lock);
	mutex_unlock(&from->lock);
	return 0;
}

int fxfs_punch(struct fxfs *fs, u64 ino, u64 offset, u64 len)
{
	struct stub_file *file = lock_file(fs, ino);
	u64 page;

	mutex_lock(&fs->lock);
	for (page = offset >> PAGE_SHIFT; page < (offset + len) >> PAGE_SHIFT &&
	     page < file->nr_blocks; page++) {
		if (ino == watched_ino && page == watched_page)
			sim_log("sim: ino %llu page %llu: punched by %s, at %llu ns\n", ino, page,
				current->comm, ktime_get_ns());
		put_block(fs, &file->blocks[page]);
	}
	mutex_unlock(&fs->lock);
	mutex_unlock(&file->lock);
	return 0;
}

/*
 * Room for `bytes` of file `ino`'s dirty pages, which writeback pays with: -ENOSPC as a
 * transaction fails (fxfs_sim_fail), when the filesystem has none, as a fault's SIGBUS and a
 * write(2)'s ENOSPC say.
 */
int fxfs_reserve_dirty(struct fxfs *fs, u64 ino, u64 bytes)
{
	struct stub_file *file;

	if (fail_odds && !(sim_random() % fail_odds)) {
		failures++;
		return -ENOSPC;
	}
	mutex_lock(&fs->lock);
	file = find_file(fs, ino);
	file->reserved += bytes;
	mutex_unlock(&fs->lock);
	return 0;
}

void fxfs_reserve_dirty_what_fits(struct fxfs *fs, u64 ino, u64 bytes)
{
	mutex_lock(&fs->lock);
	find_file(fs, ino)->reserved += bytes;
	mutex_unlock(&fs->lock);
}

/* Gives back room of `bytes` of dirty pages, dropped unwritten: what was reserved, once. */
static void unreserve(struct stub_file *file, u64 bytes, const char *what)
{
	if (bytes > file->reserved)
		sim_panic("file %llu's %s gives back %llu bytes of dirty pages' room, of %llu reserved",
			  file->ino, what, bytes, file->reserved);
	file->reserved -= bytes;
}

void fxfs_release_dirty(struct fxfs *fs, u64 ino, u64 bytes)
{
	mutex_lock(&fs->lock);
	unreserve(find_file(fs, ino), bytes, "release");
	mutex_unlock(&fs->lock);
}

u64 fxfs_sim_reserved(struct fxfs *fs, u64 ino)
{
	return find_file(fs, ino)->reserved;
}

/* A page fxfs_write_many writes: the file's, and the new block that takes its place. */
struct written {
	u64 page;
	struct block block;
};

/* How many of fxfs_write_many's next transactions fail as they commit (fxfs_set_fail_commits). */
static u64 writebacks_to_fail;

/*
 * The guest's fault injection (TASK-164.2): the next `count` writebacks fail with EIO, their bytes
 * written, before their commits, which leaves the file as a crash between the two would.
 */
void fxfs_set_fail_commits(u64 count)
{
	writebacks_to_fail = count;
}

/* Whether writeback's commits wait (fxfs_sim_hold_commits), and the one waiting, of file held_ino. */
static bool hold_commits;
static const struct written *held;
static size_t held_count;
static u64 held_ino;

void fxfs_sim_hold_commits(bool hold)
{
	hold_commits = hold;
}

bool fxfs_sim_commit_held(void)
{
	return held;
}

void fxfs_sim_read_pending(struct fxfs *fs, u64 ino, u64 offset, void *buf, size_t len)
{
	size_t i, done;

	fxfs_sim_read(fs, ino, offset, buf, len);
	if (!held || ino != held_ino)
		return;
	for (i = 0; i < held_count; i++) {
		u64 at = held[i].page << PAGE_SHIFT;
		char page[PAGE_SIZE];

		if (at + PAGE_SIZE <= offset || at >= offset + len)
			continue;
		read_block(fs, &held[i].block, page);
		for (done = 0; done < PAGE_SIZE; done++) {
			if (at + done >= offset && at + done < offset + len)
				((char *)buf)[at + done - offset] = page[done];
		}
	}
}

/*
 * fxfs_write_many: the transaction holds the file's lock throughout. Each page copied, a page at a
 * time, others running between, as the window's page may change under writeback: what it copied
 * is what it writes, and a page part of which a part covers keeps the rest of its block. Then a new
 * extent for them all, minted and written with no seal between, which would forget it open while
 * the device took it as open, until the next; then, unless a test holds it, the commit.
 */
int fxfs_write_many(struct fxfs *fs, u64 ino, const struct fxfs_write_part *parts, size_t count,
		    u64 dirty)
{
	struct stub_file *file = lock_file(fs, ino);
	struct stub_extent *extent;
	struct written *pages;
	char *copies;
	size_t i, n = 0, total = 0;
	u64 page, end = 0, store;

	/* The room, taken first, whatever the writes come to, as fxfs-posix's write_dirty takes it. */
	mutex_lock(&fs->lock);
	unreserve(file, dirty, "writeback");
	mutex_unlock(&fs->lock);
	for (i = 0; i < count; i++) {
		if (parts[i].len)
			total += ((parts[i].offset + parts[i].len - 1) >> PAGE_SHIFT) -
				 (parts[i].offset >> PAGE_SHIFT) + 1;
	}
	pages = kcalloc(max_t(size_t, total, 1), sizeof(*pages), GFP_NOFS);
	copies = kmalloc(max_t(size_t, total, 1) << PAGE_SHIFT, GFP_NOFS);
	for (i = 0; i < count; i++) {
		u64 from = parts[i].offset, to = parts[i].offset + parts[i].len;

		if (!parts[i].len)
			continue;
		end = max(end, to);
		for (page = from >> PAGE_SHIFT; page << PAGE_SHIFT < to; page++, n++) {
			u64 at = max(from, page << PAGE_SHIFT);
			u64 stop = min(to, (page + 1) << PAGE_SHIFT);
			char *copy = copies + (n << PAGE_SHIFT);

			if (at > page << PAGE_SHIFT || stop < (page + 1) << PAGE_SHIFT) {
				mutex_lock(&fs->lock);
				reserve_blocks(file, page + 1);
				read_block(fs, &file->blocks[page], copy);
				mutex_unlock(&fs->lock);
			}
			memcpy(copy + offset_in_page(at), parts[i].data + (at - from), stop - at);
			pages[n].page = page;
			sim_point("a Fxfs copy");
		}
	}
	/* One device write, which no seal comes into: a seal between would leave it unsealed. */
	down_read(&fs->writes);
	mutex_lock(&fs->lock);
	extent = n ? new_extent(fs, n) : NULL;
	for (i = 0; i < n; i++) {
		pages[i].block = (struct block) { extent->sequence, i };
		extent->live++;
		extent->refs[i]++;
	}
	mutex_unlock(&fs->lock);
	if (n) {
		sim_point("a Fxfs write");
		store = sim_device_store_offset(extent->sequence);
		host_store_write(store, copies, n << PAGE_SHIFT);
		sim_device_wrote(extent->sequence, 0, n);
		sim_point("a Fxfs write's reply");
	}
	up_read(&fs->writes);
	kfree(copies);
	/* A crash now leaves the file's old blocks, and the new ones orphans, which a test may see. */
	if (hold_commits) {
		held = pages;
		held_count = n;
		held_ino = ino;
		while (hold_commits)
			msleep(1);
		held = NULL;
	}
	sim_point("writeback's commit");
	mutex_lock(&fs->lock);
	if (writebacks_to_fail) {
		/*
		 * The new blocks named by no record, which die; the room was taken all the same.
		 */
		writebacks_to_fail--;
		failures++;
		for (i = 0; i < n; i++) {
			extent->live--;
			extent->refs[i]--;
		}
		if (extent)
			extent->died = fs->seals;
		mutex_unlock(&fs->lock);
		mutex_unlock(&file->lock);
		kfree(pages);
		return -EIO;
	}
	for (i = 0; i < n; i++) {
		page = pages[i].page;
		reserve_blocks(file, page + 1);
		if (ino == watched_ino && page == watched_page)
			sim_log("sim: ino %llu page %llu: written back by %s, from extent %llu page %llu to extent %llu page %llu, at %llu ns\n",
				ino, page, current->comm, file->blocks[page].sequence,
				file->blocks[page].page, pages[i].block.sequence,
				pages[i].block.page, ktime_get_ns());
		put_block(fs, &file->blocks[page]);
		file->blocks[page] = pages[i].block;
	}
	file->size = max(file->size, end);
	mutex_unlock(&fs->lock);
	mutex_unlock(&file->lock);
	kfree(pages);
	return 0;
}
