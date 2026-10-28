// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * Files' data mapped from the extent-store device's DAX window (backlog doc-2), as virtio-fs's DAX
 * maps a host's files (fs/fuse/dax.c), instead of the page cache: reads, writes and mappings of
 * files reach the extents the host shares, through the window.
 *
 * The window is split into ranges, each mapping a chunk of FXFS_DAX_SIZE bytes of one file: the
 * extents holding the chunk's data, page by page, read-only, or writably for open extents (doc-4),
 * and holes left out. A range is as large as its chunk of the file, in pages: a small file's takes
 * what it needs of the window, rounded up to a power of two, so that a file growing a page at a
 * time maps its data again only as often as it doubles. A write first makes room for its data in
 * open extents (fxfs_dax_map_write), which the range then maps writably, and stores through the
 * mapping; mappings of files map the window's pages. A file's page stays at one window address,
 * whatever extent holds it: a block that moves is mapped again there.
 *
 * A chunk whose data is one piece of an extent of at most a chunk's size, as small files' are,
 * sharing extents (the allocator's staging extents), is a slice of that extent's region instead:
 * the window's mapping of the whole extent, from its start, which all the files it holds share. So
 * a file written, or read, takes no request of its own, once its extent is mapped: a region of an
 * open extent is mapped writably, further than its blocks are handed out yet, doubling. A region
 * goes with its last slice. A write a slice can't take where it is, as its blocks move, drops the
 * slice (-EXDEV), and the chunk is mapped again.
 *
 * So too a range's pages whose blocks are in such extents, as blocks a write moves to the staging
 * extent are: the range maps them where their extent's region does (fxfs_dax_move), rather than
 * in its own space, so that a block moved takes no MAP of its own, and the window maps each extent
 * once. A page that moves so is at another address: the iomaps of it after it moved say it is
 * shared (IOMAP_F_SHARED), so that the kernel replaces its mappings of the page.
 *
 * Ranges belong to a file (struct fxfs_dax_file, by inode number), not to an inode: they outlive
 * the inodes read in for the file, so that a file read again maps from the window what it mapped
 * before, with no request, as pmem stays mapped across the guest's dropped caches. A file's ranges
 * go as its data does (unlinked, truncated); others are reclaimed, least recently used, when the
 * window runs out.
 *
 * Requests that seal open extents make their writable mappings read-only. Stores are made in the
 * device's gate (device.h), which such requests wait for; before them, the sealer makes user
 * mappings of writable pages read-only, so that a store faults, and forgets which pages are
 * writable, so that a write makes room again. Kernel copies through the window are made with page
 * faults disabled, as one waiting for the gate to open may hold what a seal needs.
 *
 * Locks are taken in this order: an inode's lock (read and write); fxdax: a follower's fill
 * barrier (vfs.h), which a batch takes holding no inode's lock; an inode's invalidate_lock
 * (faults, truncation and reclaim); then its file's DAX semaphore, then the gate. Another inode's
 * are only tried, when reclaiming its ranges. Rust is called holding no gate.
 */

#include <linux/dax.h>
#include <linux/delay.h>
#include <linux/genalloc.h>
#include <linux/hash.h>
#include <linux/hashtable.h>
#include <linux/iomap.h>
#include <linux/log2.h>
#include <linux/mm.h>
#include <linux/moduleparam.h>
#include <linux/pagemap.h>
#include <linux/pagewalk.h>
#include <linux/prandom.h>
#include <linux/seq_file.h>
#include <linux/slab.h>
#include <linux/uio.h>
#include <linux/wait.h>
#include <linux/workqueue.h>
#include <linux/writeback.h>

#include "device.h"
#include "vfs.h"

/* A range's size: a power of two, the window's and chunks' alignment. */
#define FXFS_DAX_SHIFT 21
#define FXFS_DAX_SIZE (1ULL << FXFS_DAX_SHIFT)
#define FXFS_DAX_PAGES (FXFS_DAX_SIZE >> PAGE_SHIFT)
/* fxdax: a sustained, unsealed append stream reserves this many bytes at a time (TASK-92.58). */
#define FXFS_DAX_APPEND_SIZE (16UL << PAGE_SHIFT)

/*
 * The window's space is handed out from a pool, page by page, at this offset: the pool takes no
 * address 0.
 */
#define FXFS_DAX_POOL_BASE (1UL << 48)

/* The most pages a writable region's mapping grows by at once. */
#define FXFS_DAX_REGION_STEP 64UL
/*
 * The pages a writable region's first MAP maps: most of a millisecond each, which a small file's
 * write waits for, so more than a page, though they make the extent at least that long.
 */
#define FXFS_DAX_REGION_FIRST 16UL

/*
 * fxdax: fault injection: a fault's copy of the page it holds fails to allocate, as it may when
 * memory is short, and Fxfs keeps the page's contents through the device instead (keep, in
 * fxfs_dax_iomap_write; TASK-79.107, TASK-99).
 */
static bool fxfs_dax_fail_held;
module_param_named(dax_fail_held, fxfs_dax_fail_held, bool, 0644);
MODULE_PARM_DESC(dax_fail_held, "Fail every DAX fault's held copy of its page (fault injection)");

/*
 * fxdax: fault injection: a write fault that makes room for its page in Fxfs seals first, holding
 * the file's semaphore, as Fxfs's map_write may wait for compaction, whose seal waits for the
 * stores in the gate (TASK-79.12).
 */
static bool fxfs_dax_fault_seals;
module_param_named(dax_fault_seals, fxfs_dax_fault_seals, bool, 0644);
MODULE_PARM_DESC(dax_fault_seals, "Seal in every DAX write fault making room, holding the file's semaphore (fault injection)");

/*
 * fxdax: fault injection: the next this many write faults whose blocks Fxfs moved seal right after
 * the move, as a seal may come between it and the fault's mapping of the new block (TASK-79.11).
 */
static unsigned int fxfs_dax_fault_seals_moved;
module_param_named(dax_fault_seals_moved, fxfs_dax_fault_seals_moved, uint, 0644);
MODULE_PARM_DESC(dax_fault_seals_moved, "Seal right after the next this many DAX write faults move their blocks (fault injection)");

/* fxdax: whether to seal after a fault's move, as fxfs.dax_fault_seals_moved asks, counting it. */
static bool fxfs_dax_seal_moved(void)
{
	unsigned int left = READ_ONCE(fxfs_dax_fault_seals_moved);

	while (left) {
		unsigned int was = cmpxchg(&fxfs_dax_fault_seals_moved, left, left - 1);

		if (was == left)
			return true;
		left = was;
	}
	return false;
}

/*
 * fxdax: delay injection (TASK-44.11): at the steps of the DAX protocol where a check and its use
 * race what other tasks change, a quarter of the time, a sleep of up to fxfs.dax_delay_us
 * microseconds, from a generator seeded with fxfs.dax_delay_seed, which a run names: a window of
 * microseconds two tasks rarely meet in, as TASK-99's, widened so that they do. Off (0) unless set.
 */
static unsigned int fxfs_dax_delay_us;
module_param_named(dax_delay_us, fxfs_dax_delay_us, uint, 0644);
MODULE_PARM_DESC(dax_delay_us, "Sleep up to this many microseconds at the DAX protocol's racing steps (delay injection)");
static unsigned long fxfs_dax_delay_seed;
module_param_named(dax_delay_seed, fxfs_dax_delay_seed, ulong, 0444);
MODULE_PARM_DESC(dax_delay_seed, "The seed of fxfs.dax_delay_us's delays");

/*
 * fxdax: diagnostics (TASK-121): whether seals' staging extents' pages are evacuated to their
 * ranges' own space, and whether ranges of their own map pieces of staging extents where the
 * extents' regions do (fxfs_dax_move), rather than in their own space: each off, a failing check
 * says whether its path is the one losing data.
 */
static bool fxfs_dax_evacuates = true;
module_param_named(dax_evacuates, fxfs_dax_evacuates, bool, 0644);
MODULE_PARM_DESC(dax_evacuates, "Evacuate sealed staging extents' pages (diagnostic)");
/*
 * fxdax: diagnostic (TASK-121): after each seal's sealer, checks that no range of any file has a
 * page writable still, which the sealer would have kept the user's mappings writable of, while
 * the device makes it read-only: the stores the window refused.
 */
static bool fxfs_dax_check_seals;
module_param_named(dax_check_seals, fxfs_dax_check_seals, bool, 0644);
MODULE_PARM_DESC(dax_check_seals, "WARN of pages left writable by a seal (diagnostic)");
/*
 * fxdax: diagnostic (TASK-121): a write moving a page of one page copies it, as the window maps
 * it, first, and WARNs if the page where it moved holds other data, but for the write's own bytes:
 * the copy the move kept of the page wasn't the page's.
 */
static bool fxfs_dax_check_moves;
module_param_named(dax_check_moves, fxfs_dax_check_moves, bool, 0644);
MODULE_PARM_DESC(dax_check_moves, "WARN of a page moved holding other data (diagnostic)");
static bool fxfs_dax_moves = true;
module_param_named(dax_moves, fxfs_dax_moves, bool, 0644);
MODULE_PARM_DESC(dax_moves, "Map staging extents' pieces where their regions do (diagnostic)");

/*
 * fxdax: pause points (TASK-119.9): a task reaching a step whose bit fxfs.dax_pause sets waits
 * there until the bit is cleared, the bit set in fxfs.dax_paused meanwhile: a check plays an
 * interleaving out exactly, where fxfs.dax_delay_us's random sleeps only make it likely.
 */
enum fxfs_dax_step {
	/* A fault's blocks moved, before it enters the gate. */
	FXFS_DAX_STEP_MOVED,
	/* A seal's sealer, the seal in progress. */
	FXFS_DAX_STEP_SEALING,
	/* A fault whose blocks moved found the gate shut by a seal. */
	FXFS_DAX_STEP_SHUT,
	/* A fault about to copy the page it holds. */
	FXFS_DAX_STEP_HELD,
};
static unsigned long fxfs_dax_pause_steps;
module_param_named(dax_pause, fxfs_dax_pause_steps, ulong, 0644);
MODULE_PARM_DESC(dax_pause, "Steps (bits) of the DAX protocol tasks wait at, until cleared (pause points)");
static unsigned long fxfs_dax_paused_steps;
module_param_named(dax_paused, fxfs_dax_paused_steps, ulong, 0444);
MODULE_PARM_DESC(dax_paused, "Steps (bits) a task waits at (pause points)");

/* fxdax: waits at `step` while fxfs.dax_pause asks. Where the caller may sleep. */
static void fxfs_dax_pause(enum fxfs_dax_step step)
{
	if (likely(!(READ_ONCE(fxfs_dax_pause_steps) & BIT(step))))
		return;
	set_bit(step, &fxfs_dax_paused_steps);
	while (READ_ONCE(fxfs_dax_pause_steps) & BIT(step))
		msleep(1);
	clear_bit(step, &fxfs_dax_paused_steps);
}

static DEFINE_SPINLOCK(fxfs_dax_delay_lock);
static struct rnd_state fxfs_dax_delay_state;
static bool fxfs_dax_delay_seeded;

/* fxdax: a delay injected here, if fxfs.dax_delay_us asks for them. Where the caller may sleep. */
static void fxfs_dax_delay(void)
{
	unsigned int most = READ_ONCE(fxfs_dax_delay_us);
	u32 chance;

	if (!most)
		return;
	spin_lock(&fxfs_dax_delay_lock);
	if (!fxfs_dax_delay_seeded) {
		prandom_seed_state(&fxfs_dax_delay_state, fxfs_dax_delay_seed);
		fxfs_dax_delay_seeded = true;
	}
	chance = prandom_u32_state(&fxfs_dax_delay_state);
	spin_unlock(&fxfs_dax_delay_lock);
	if (chance % 4)
		return;
	fsleep(chance / 4 % most + 1);
}

/* The window's mapping of a whole extent, from its start, which slices of files share. */
struct fxfs_dax_region {
	/* In the filesystem's regions, by extent. */
	struct hlist_node node;
	u64 branch;
	u64 sequence;
	u64 window_offset;
	/* Its space in the window, in pages: as far as the extent reaches, or may yet. */
	unsigned long pages;
	/*
	 * Its slices, the ranges whose pages it maps (fxfs_dax_move), and its growth. Guarded by the
	 * regions lock.
	 */
	unsigned int refs;
	/* Guards `mapped`, and the region's MAPs. */
	struct mutex lock;
	/* How many of its pages are mapped, from its start. */
	unsigned long mapped;
	/*
	 * Whether it maps its extent writably, while it is open: with the gate closed, a seal makes it
	 * read-only. Set in the gate, under the regions lock.
	 */
	bool writable;
	/* On the filesystem's writable regions, while it is. Guarded by the regions lock. */
	struct list_head writable_list;
	/*
	 * Maps a writable region further ahead of the writes that will use it, so that they don't
	 * wait for the MAP: while `growing`, it holds a reference to the region.
	 */
	struct fxfs_dax *dax;
	struct work_struct grow;
	atomic_t growing;
	/* The furthest page writes reached, which the region grows ahead of. */
	atomic_long_t reached;
	/*
	 * fxdax: whether a seal sealed it writable, a staging extent's: the pages ranges moved there
	 * move to the ranges' own space after (fxfs_dax_evacuate_work). Set under the regions lock.
	 */
	bool evacuate;
};

/* A file's ranges of the window, which outlive its inodes. */
struct fxfs_dax_file {
	u64 ino;
	/* fxdax: the append stream since the last seal, under the inode's write lock (TASK-92.58). */
	loff_t append_next;
	u64 append_bytes, append_seals;
	/* The filesystem's (`files`), and others' while they use it. */
	refcount_t refs;
	/* The inode read in for the file, if there is one. Guarded by the filesystem's files lock. */
	struct inode *inode;
	/* Guards `ranges`, and which pages of them are mapped. */
	struct rw_semaphore sem;
	/* The window's ranges mapping chunks of the file, by chunk. */
	struct xarray ranges;
	/* On the filesystem's list of files with writable window pages, if it has some. */
	struct list_head writable;
	/*
	 * fxdax: on the filesystem's list of files whose pages a seal sealed in staging extents'
	 * regions, which hold a reference to them (fxfs_dax_evacuate_work). Guarded by its writable
	 * lock.
	 */
	struct list_head evacuating;
	/*
	 * fxdax: how many of its ranges an evacuation holds (fxfs_dax_evacuee_gather): ranges that
	 * the paths dropping them whatever uses them, evict and truncate, wait for, so that what the
	 * evacuation maps doesn't land in window space another range took (fxfs_dax_lock_unheld).
	 * Changed under its semaphore.
	 */
	atomic_t held;
};

/* A range of the window. */
struct fxfs_dax_range {
	/* On the filesystem's used list. */
	struct list_head list;
	/* The file whose chunk it maps, and which. */
	struct fxfs_dax_file *file;
	pgoff_t chunk;
	u64 window_offset;
	/* The region it is a slice of, whose pages it maps, if it is one. */
	struct fxfs_dax_region *region;
	/* Its size, in pages: its chunk's, up to the file's end when it was mapped. */
	unsigned long pages;
	/* The iomaps using it, which keep it. */
	atomic_t users;
	/* Its pages that map an extent, and those that map one writably. */
	unsigned long *mapped;
	unsigned long *writable;
	/*
	 * A range of its own's pages mapped where their extents' regions map them (fxfs_dax_move):
	 * for each page, 0 if it is in the range's own space, or else the index of its region in
	 * `regions` plus one, shifted 32 bits, and its page in the region. Made as a page first moves.
	 */
	u64 *moved;
	/*
	 * The regions its pages moved to, each of which it holds a reference to while any page is
	 * there, or may be used there: iomaps may use a page where it was until they end, and the
	 * kernel's mappings of it until the kernel replaces them (fxfs_dax_prune).
	 */
	struct fxfs_dax_region **regions;
	unsigned int nr_regions;
	/*
	 * Its pages at another address than the iomaps of them said before, which the next iomaps
	 * say are shared (fxfs_dax_cow).
	 */
	unsigned long *relocated;
	/*
	 * fxdax: whether a fault mapped its pages 2 MiB at once, in a row in the window, with one PMD,
	 * one DAX entry for them all: a page of them moving breaks the row up, and relocates them
	 * all (fxfs_dax_relocate), so that the faults of each replace the entry. Only the moved
	 * page's did, and the others' met the PMD entry, which a page's fault can't take: SIGBUS
	 * (TASK-123).
	 */
	bool huge;
	/*
	 * Freed after a grace period: the sealer walks files' ranges without their semaphores,
	 * which writers hold waiting for the gate the sealer keeps closed.
	 */
	struct rcu_head rcu;
};

/*
 * fxdax: why the window maps: a region as far as a write or a fault needs, ahead of the writes
 * in the background, the staging extent's spare, a piece in a range's own space (TASK-91.4), or
 * pages a seal sealed in a staging extent's region in their ranges' own space (TASK-91.10). The
 * first and the fourth are for the access at hand, which the device maps ahead of (MAP_AHEAD,
 * TASK-112.1); evacuated pages, written and done with, mostly aren't touched again.
 */
enum fxfs_dax_why {
	FXFS_DAX_REACH,
	FXFS_DAX_GROW,
	FXFS_DAX_RESTAGE,
	FXFS_DAX_RANGE,
	FXFS_DAX_EVACUATE,
	FXFS_DAX_WHYS,
};

static const char *const fxfs_dax_whys[FXFS_DAX_WHYS] = {
	"reach", "grow", "restage", "range", "evacuate",
};

struct fxfs_dax {
	struct super_block *sb;
	struct fxfs *fs;
	struct fxfs_device *dev;
	struct dax_device *dax_dev;
	/* The window's free space for ranges of their own; regions' too, unless they have theirs. */
	struct gen_pool *pool;
	/*
	 * fxdax: the window's free space for regions: its last part, from `regions_at`, apart from the
	 * ranges' (TASK-91.10), or `pool`, in a window too small to split.
	 */
	struct gen_pool *region_pool;
	u64 regions_at;
	/*
	 * fxdax: how often the window had no room for a range, or for a region in their own space,
	 * how many ranges were reclaimed for room, how many regions there are, and how many ranges
	 * let go of were (TASK-91.7, TASK-91.10).
	 */
	atomic_long_t window_full, regions_full, reclaims, region_count, regions_pruned;
	/* fxdax: the window's MAPs, and the bytes they mapped, by why (`enum fxfs_dax_why`). */
	atomic64_t maps[FXFS_DAX_WHYS], map_bytes[FXFS_DAX_WHYS];
	/* fxdax: the writes that made room through a Fxfs transaction (TASK-92.28). */
	atomic64_t map_writes;
	/* Requested bytes of extent-result buffers, before allocator rounding. */
	atomic64_t extent_alloc_bytes;
	/*
	 * fxdax: the writes that waited for their file's semaphore, and how long, in ns; and the
	 * evacuations of a file with runs, how long they held its semaphore, and how long their MAPs
	 * took of that (TASK-91.16).
	 */
	atomic64_t write_waits, write_wait_ns, evacuations, evacuate_hold_ns, evacuate_map_ns;
	/*
	 * fxdax: the writes whose blocks Fxfs moved before a seal kept them from the gate, which
	 * their ranges map read-only where they moved to (fxfs_dax_map_moved, TASK-121).
	 */
	atomic64_t moved_unsealed;
	/* The files with ranges, or inodes using the window, by inode number. */
	struct xarray files;
	/*
	 * fxdax: with fxfs.dax_shadow, the window's pages as the guest mapped them last, by page:
	 * the device's seal count then, shifted, and whether writably (TASK-121).
	 */
	struct xarray shadow;
	/* Guards `files`, and files' inodes. */
	struct mutex files_lock;
	/* Guards the used list. */
	spinlock_t lock;
	/* The ranges mapping chunks, least recently used first. */
	struct list_head used;
	/* Woken as ranges are freed, or their last users done, which counts in `released`. */
	wait_queue_head_t wait;
	atomic_long_t released;
	/* Guards `writable`: the files with writable pages in the window. */
	struct mutex writable_lock;
	struct list_head writable;
	/* The regions mapping extents, by extent. */
	DECLARE_HASHTABLE(regions, 12);
	/* Guards `regions`, regions' references, and the writable regions. */
	spinlock_t regions_lock;
	struct list_head writable_regions;
	/* Where writable regions grow ahead of their writes (fxfs_dax_grow), and how many do. */
	struct workqueue_struct *grow_wq;
	atomic_t growing;
	/*
	 * fxdax: the staging extent's region, mapped once a seal ends, before the next writes' blocks
	 * move there (fxfs_dax_restage_work): a reference to it, until the next. Guarded by the regions
	 * lock.
	 */
	struct fxfs_dax_region *spare;
	struct work_struct restage;
	/*
	 * fxdax: the files whose pages seals sealed in staging extents' regions, which move to their
	 * ranges' own space once a seal ends (fxfs_dax_evacuate_work). Guarded by the writable lock.
	 */
	struct list_head evacuating;
	struct work_struct evacuate;
	/* Whether seals restage: until the filesystem unmounts. */
	bool restaging;
};

static const struct iomap_ops fxfs_dax_iomap_ops;

/*
 * fxdax: diagnostic (TASK-121): the guest keeps a shadow of how it mapped each page of the window
 * (fxfs_dax_shadow_map), and WARNs of a write's iomap mapping a page writably that it mapped
 * read-only last, or before the last seal, which made it read-only: a store there the window
 * refuses.
 */
static bool fxfs_dax_shadowed;
module_param_named(dax_shadow, fxfs_dax_shadowed, bool, 0644);
MODULE_PARM_DESC(dax_shadow, "WARN of writes to pages the window maps read-only (diagnostic)");

/* fxdax: records `len` bytes at `window_offset` as mapped, writably if `writable`, or unmapped. */
static void fxfs_dax_shadow_map(struct fxfs_dax *dax, u64 window_offset, u64 len, bool writable,
				bool mapped)
{
	u64 seals = fxfs_device_seals(dax->dev);
	unsigned long page;

	if (!READ_ONCE(fxfs_dax_shadowed))
		return;
	for (page = window_offset >> PAGE_SHIFT; page < (window_offset + len) >> PAGE_SHIFT; page++) {
		void *entry = xa_load(&dax->shadow, page);
		u64 then = entry ? xa_to_value(entry) >> 1 : 0;

		/*
		 * A page mapped writably since the last seal, or before one begun and waiting, which
		 * the user's mappings may map writably still: mapped read-only, or unmapped, their
		 * stores are refused.
		 */
		if (mapped && !writable && entry && (xa_to_value(entry) & 1) &&
		    (then == seals || ((seals & 1) && then == seals - 1)))
			pr_err("fxfs: window page %lx, mapped writably at seals %llu, mapped read-only at %llu by %pS\n",
			       page, then, seals, __builtin_return_address(1));
		if (mapped)
			xa_store(&dax->shadow, page, xa_mk_value((seals << 1) | writable), GFP_NOFS);
		else
			xa_erase(&dax->shadow, page);
	}
}

static int fxfs_dax_device_map(struct fxfs_dax *dax, u64 branch, u64 sequence, u64 offset,
			       u64 len, u64 window_offset, bool writable, bool ahead)
{
	int err = fxfs_device_map(dax->dev, branch, sequence, offset, len, window_offset, writable,
				  ahead);

	if (!err)
		fxfs_dax_shadow_map(dax, window_offset, len, writable, true);
	return err;
}

static int fxfs_dax_device_map_many(struct fxfs_dax *dax, const struct fxfs_device_map *maps,
				    unsigned int count)
{
	int err = fxfs_device_map_many(dax->dev, maps, count);
	unsigned int i;

	for (i = 0; !err && i < count; i++)
		fxfs_dax_shadow_map(dax, maps[i].window_offset, maps[i].len, maps[i].writable, true);
	return err;
}

static int fxfs_dax_device_unmap(struct fxfs_dax *dax, u64 window_offset, u64 len)
{
	int err = fxfs_device_unmap(dax->dev, window_offset, len);

	fxfs_dax_shadow_map(dax, window_offset, len, false, false);
	return err;
}

static u64 fxfs_dax_page_offset(const struct fxfs_dax_range *range, unsigned long page);

/*
 * fxdax: WARNs unless the window maps `range`'s `pages` pages from `page` writably, as the
 * shadow has them, since the last seal: a write's iomap that takes them for writable (TASK-121).
 */
static void fxfs_dax_shadow_check(struct fxfs_dax *dax, struct fxfs_dax_range *range,
				  unsigned long page, unsigned long pages, const char *why)
{
	u64 seals = fxfs_device_seals(dax->dev);
	unsigned long i, at;
	void *entry;

	if (!READ_ONCE(fxfs_dax_shadowed))
		return;
	for (i = page; i < page + pages; i++) {
		at = fxfs_dax_page_offset(range, i) >> PAGE_SHIFT;
		entry = xa_load(&dax->shadow, at);
		/*
		 * Mapped writably since the last seal; or before one that has begun (odd), which waits
		 * for this store in the gate to end before it seals.
		 */
		WARN(!entry || !(xa_to_value(entry) & 1) ||
		     ((xa_to_value(entry) >> 1) != seals &&
		      !((seals & 1) && (xa_to_value(entry) >> 1) == seals - 1)),
		     "fxfs: %s: ino %llu chunk %lu page %lu writable at window page %lx, which the guest %s (seals %llu, then %lu, %s %llx)\n",
		     why, range->file->ino, range->chunk, i, at,
		     !entry ? "never mapped" : !(xa_to_value(entry) & 1) ? "mapped read-only last" :
		     "mapped writably before a seal",
		     seals, entry ? xa_to_value(entry) >> 1 : 0,
		     range->region ? "a slice of region" : range->moved && range->moved[i] ?
		     "moved to region" : "in its own space at",
		     range->region ? range->region->window_offset :
		     range->moved && range->moved[i] ?
		     range->regions[(range->moved[i] >> 32) - 1]->window_offset : range->window_offset);
	}
}
static const struct address_space_operations fxfs_dax_aops;
static const struct file_operations fxfs_dax_file_operations;
static int fxfs_dax_break_layouts(struct inode *inode, loff_t start);
static int fxfs_dax_reach(struct fxfs_dax *dax, struct fxfs_dax_region *region, unsigned long end);
static void fxfs_dax_put_region(struct fxfs_dax *dax, struct fxfs_dax_region *region);
static void fxfs_dax_grow(struct work_struct *work);
static void fxfs_dax_restage_work(struct work_struct *work);
static void fxfs_dax_evacuate_work(struct work_struct *work);
static int fxfs_dax_move(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			 const struct fxfs_dax_extent *e, unsigned long page, unsigned long pages);

static struct fxfs_dax *fxfs_dax_of(struct inode *inode)
{
	return fxfs_vfs_info(inode->i_sb)->dax;
}

/* The file `inode` is read in for, whose ranges it uses. */
static struct fxfs_dax_file *fxfs_dax_file(struct inode *inode)
{
	return fxfs_vfs_inode(inode)->dax.file;
}

static loff_t fxfs_dax_chunk_start(struct fxfs_dax_range *range)
{
	return (loff_t)range->chunk << FXFS_DAX_SHIFT;
}

/* Room for the pieces of a chunk's data, at most one per page, from Rust. */
static struct fxfs_dax_extent *fxfs_dax_alloc_extents(struct fxfs_dax *dax, size_t capacity)
{
	struct fxfs_dax_extent *extents;

	/* DAX blocks are pages: at most one returned extent per covered logical page. */
	extents = fxfs_nofs(kvmalloc_array(capacity, sizeof(*extents), GFP_KERNEL));
	if (extents)
		atomic64_add(capacity * sizeof(*extents), &dax->extent_alloc_bytes);
	return extents;
}

/* Marks `file` as having writable pages in the window, which the sealer forgets. In the gate. */
static void fxfs_dax_add_writable(struct fxfs_dax *dax, struct fxfs_dax_file *file)
{
	fxfs_device_assert_in_gate(dax->dev);

	mutex_lock(&dax->writable_lock);
	if (list_empty(&file->writable))
		list_add_tail(&file->writable, &dax->writable);
	mutex_unlock(&dax->writable_lock);
}

/*
 * Takes the pieces `extents` of the chunk the slice `range` maps where it maps them, writably those
 * of its region's extent while it is open, which is mapped as far as they reach; or -EXDEV if they
 * are elsewhere, as moved blocks are, which the slice can't map. In the gate, with the file's DAX
 * semaphore held for writing.
 */
static int fxfs_dax_map_slice(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			      const struct fxfs_dax_extent *extents, ssize_t count)
{
	struct fxfs_dax_region *region = range->region;
	loff_t start = fxfs_dax_chunk_start(range);
	/* Where the slice starts in its extent. */
	u64 base = range->window_offset - region->window_offset;
	bool writable = false;
	ssize_t i;
	int err;

	fxfs_device_assert_in_gate(dax->dev);
	lockdep_assert_held_write(&range->file->sem);
	for (i = 0; i < count; i++) {
		const struct fxfs_dax_extent *e = &extents[i];
		unsigned long page = (e->offset - start) >> PAGE_SHIFT;
		unsigned long pages = e->len >> PAGE_SHIFT;
		u64 end = base + (e->offset - start) + e->len;

		if (e->branch != region->branch || e->sequence != region->sequence ||
		    e->extent_offset != base + (e->offset - start) || page + pages > range->pages ||
		    (e->open && !region->writable))
			return -EXDEV;
		if (!e->open)
			continue;
		err = fxfs_dax_reach(dax, region, end >> PAGE_SHIFT);
		if (err)
			return err;
		bitmap_set(range->writable, page, pages);
		writable = true;
	}
	if (writable)
		fxfs_dax_add_writable(dax, range->file);
	return 0;
}

/* Where `range`'s page `page` is in the window: in its own space, or its region's (`moved`). */
static u64 fxfs_dax_page_offset(const struct fxfs_dax_range *range, unsigned long page)
{
	u64 moved = range->moved ? range->moved[page] : 0;

	if (!moved)
		return range->window_offset + ((u64)page << PAGE_SHIFT);
	return range->regions[(moved >> 32) - 1]->window_offset + ((moved & U32_MAX) << PAGE_SHIFT);
}

/* How many of `range`'s pages from `page` on, before `end`, are in a row in the window. */
static unsigned long fxfs_dax_run(const struct fxfs_dax_range *range, unsigned long page,
				  unsigned long end)
{
	u64 at = fxfs_dax_page_offset(range, page);
	unsigned long next;

	if (!range->moved)
		return end - page;
	for (next = page + 1; next < end; next++) {
		if (fxfs_dax_page_offset(range, next) != at + ((u64)(next - page) << PAGE_SHIFT))
			break;
	}
	return next - page;
}

/*
 * Has `range`'s `pages` pages from `page` on be where `region`'s pages from `first` on are, or in
 * the range's own space without `region`: those the window mapped elsewhere before are
 * relocated, so that the kernel's mappings of them go (fxfs_dax_cow). With the file's DAX
 * semaphore held for writing.
 */
static void fxfs_dax_relocate(struct fxfs_dax_range *range, unsigned long page,
			      unsigned long pages, struct fxfs_dax_region *region, unsigned long first)
{
	unsigned int index = 0;
	bool changed = false;
	unsigned long i;
	u64 moved;

	lockdep_assert_held_write(&range->file->sem);

	if (!range->moved)
		return;
	if (region) {
		while (range->regions[index] != region)
			index++;
	}
	for (i = 0; i < pages; i++) {
		moved = region ? ((u64)(index + 1) << 32) | (first + i) : 0;
		if (range->moved[page + i] != moved && test_bit(page + i, range->mapped)) {
			set_bit(page + i, range->relocated);
			changed = true;
		}
		range->moved[page + i] = moved;
	}
	/* fxdax: a row a PMD mapped broken up: every page of it is relocated (TASK-123). */
	if (changed && READ_ONCE(range->huge)) {
		bitmap_or(range->relocated, range->relocated, range->mapped, range->pages);
		WRITE_ONCE(range->huge, false);
	}
}

/*
 * Has `iomap` say its pages are shared, if any of `range`'s `pages` pages from `page`, which it
 * maps, was relocated since the faults before: so that the kernel replaces its DAX entries of
 * them, which say where the pages were, and its mappings of them (dax_insert_entry,
 * dax_iomap_iter, dax_zero_iter), copying nothing: `srcmap`, the data around a write, is where the
 * pages are already. Fills in `srcmap`, so after `iomap` is. (fxdax: the kernel copied the pages
 * onto themselves, which undid stores racing the copy through other mappings of the pages: it
 * copies nothing where the source is the destination, patches/0006, TASK-122.)
 *
 * A fault's iomap (`flags`) forgets they were relocated: the fault replaces its page's entry, in
 * the entry's lock, before anything maps the page again. Another's, a write(2)'s or zeroing's,
 * replaces them only once it has copied, which faults of the pages may come before, and which
 * the kernel would otherwise have map the pages where their entries say, where they were
 * (validate_page_before_insert, insert_page_into_pte_locked): they stay relocated for the faults
 * after, unless no entry is of them, so that nothing maps them, nor is a fault of them under way
 * (faults make their entries before their iomaps).
 */
static void fxfs_dax_cow(struct inode *inode, struct fxfs_dax_range *range, unsigned long page,
			 unsigned long pages, unsigned int flags, struct iomap *iomap,
			 struct iomap *srcmap)
{
	pgoff_t index = (fxfs_dax_chunk_start(range) >> PAGE_SHIFT) + page;
	bool fault = flags & IOMAP_FAULT;
	bool relocated = false;
	unsigned long i;

	if (find_next_bit(range->relocated, page + pages, page) == page + pages)
		return;
	if (fault || !xa_find(&inode->i_mapping->i_pages, &index, index + pages - 1, XA_PRESENT)) {
		/* Atomically: read iomaps take the file's semaphore for reading. */
		for (i = page; i < page + pages; i++)
			relocated |= test_and_clear_bit(i, range->relocated);
		/* Without entries, there are none to replace. */
		if (!fault || !relocated)
			return;
	}
	iomap->flags |= IOMAP_F_SHARED;
	*srcmap = *iomap;
	srcmap->flags = 0;
	srcmap->private = NULL;
}

/*
 * fxdax: lets go of the regions none of the range of its own `range`'s pages is in any more, as
 * pages written again move to the staging extent's region after every seal: the range held every
 * region its pages were ever in, and under a database fsyncing at each transaction, the window
 * filled with thousands of them (TASK-91.10). Only while no other iomap uses the range, which
 * may use a page where it was, and the kernel's mappings are of where its pages are, none
 * relocated (fxfs_dax_cow). With the file's DAX semaphore held for writing, and an iomap's use
 * of the range.
 */
static void fxfs_dax_prune(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	unsigned int i, kept = 0, nr = range->nr_regions;
	unsigned long page, *used;
	u64 moved;

	lockdep_assert_held_write(&range->file->sem);

	if (!nr || atomic_read(&range->users) != 1 ||
	    !bitmap_empty(range->relocated, range->pages))
		return;
	used = bitmap_zalloc(nr, GFP_NOFS);
	if (!used)
		return;
	for (page = 0; page < range->pages; page++) {
		if (range->moved[page])
			__set_bit((range->moved[page] >> 32) - 1, used);
	}
	if (bitmap_full(used, nr)) {
		bitmap_free(used);
		return;
	}
	/* Its pages' regions' indices, as those before them it lets go of go. */
	for (page = 0; page < range->pages; page++) {
		moved = range->moved[page];
		if (moved)
			range->moved[page] = ((u64)(bitmap_weight(used, (moved >> 32) - 1) + 1) << 32) |
					     (moved & U32_MAX);
	}
	for (i = 0; i < nr; i++) {
		if (test_bit(i, used)) {
			range->regions[kept++] = range->regions[i];
		} else {
			fxfs_dax_put_region(dax, range->regions[i]);
			atomic_long_inc(&dax->regions_pruned);
		}
	}
	range->nr_regions = kept;
	bitmap_free(used);
}

/*
 * fxdax: makes `range`'s `pages` pages from `page` on not writable, a bit at a time, atomically:
 * outside the gate, as the sealer clears them all locklessly (fxfs_dax_seal), a word written back
 * whole could set again bits it had just cleared, and a store went to a sealed page (TASK-79.50).
 */
static void fxfs_dax_clear_writable(struct fxfs_dax_range *range, unsigned long page,
				    unsigned long pages)
{
	unsigned long i;

	for (i = page; i < page + pages; i++)
		clear_bit(i, range->writable);
}

/*
 * fxdax: has `range` say piece `e` of its chunk is mapped, as mapping it did (`err`): writably if
 * it is open. A sealed extent the device doesn't hold was never written: its blocks read as zeros,
 * a hole, as Fxfs reads them. (Blocks a write allocated before a seal sealed their extent
 * unwritten, which the write then moves.) Whether it is writable, or the error.
 */
static int fxfs_dax_mapped(struct fxfs_dax_range *range, const struct fxfs_dax_extent *e,
			   int err)
{
	loff_t start = fxfs_dax_chunk_start(range);
	unsigned long page = (e->offset - start) >> PAGE_SHIFT;
	unsigned long pages = e->len >> PAGE_SHIFT;

	if (err == -ENOENT && !e->open)
		return 0;
	if (err) {
		pr_warn("fxfs: mapping %llu bytes at %llu of extent %llx:%llu, %s, at %llu of the DAX window failed: %d\n",
			e->len, e->extent_offset, e->branch, e->sequence,
			e->open ? "writably" : "read-only",
			range->window_offset + (e->offset - start), err);
		return err;
	}
	bitmap_set(range->mapped, page, pages);
	if (e->open)
		bitmap_set(range->writable, page, pages);
	else
		fxfs_dax_clear_writable(range, page, pages);
	return e->open;
}

/* fxdax: maps piece `e` of `range`'s chunk in the range's own space, alone. */
static int fxfs_dax_map_own(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			    const struct fxfs_dax_extent *e)
{
	loff_t start = fxfs_dax_chunk_start(range);
	int err;

	err = fxfs_dax_device_map(dax, e->branch, e->sequence, e->extent_offset, e->len,
			      range->window_offset + (e->offset - start), e->open, true);
	atomic64_inc(&dax->maps[FXFS_DAX_RANGE]);
	atomic64_add(e->len, &dax->map_bytes[FXFS_DAX_RANGE]);
	if (!err)
		fxfs_dax_relocate(range, (e->offset - start) >> PAGE_SHIFT, e->len >> PAGE_SHIFT,
				  NULL, 0);
	return err;
}

/*
 * Maps the pieces `extents` of the chunk `range` maps, at their places in it: open extents
 * writably. A range of its own maps a piece of part of an extent no larger than a chunk, as files
 * share the staging extents, where the extent's region does, if it can (fxfs_dax_move). fxdax: a
 * whole extent, the file's own, it maps in its own space, in a row with the rest of the chunk,
 * which a fault then maps at once (fxfs_dax_huge_fault, TASK-26.18.8); those pieces all in one
 * request, if the device takes them so (fxfs_device_map_many, TASK-91.4), or one by one if that
 * fails, to say which did. In the gate if any is open, with the file's DAX semaphore held for
 * writing.
 */
static int fxfs_dax_map_extents(struct fxfs_dax *dax, struct fxfs_dax_range *range,
				const struct fxfs_dax_extent *extents, ssize_t count)
{
	loff_t start = fxfs_dax_chunk_start(range);
	/* The pieces the range's own space maps, and which they are, if it can gather them. */
	struct fxfs_device_map *maps = NULL;
	unsigned int *own = NULL;
	bool writable = false;
	ssize_t i, nr_own = 0;
	int err, many;

	lockdep_assert_held_write(&range->file->sem);
	if (range->region)
		return fxfs_dax_map_slice(dax, range, extents, count);
	fxfs_dax_prune(dax, range);
	if (count > 1) {
		maps = kmalloc_array(count, sizeof(*maps), GFP_NOFS);
		own = kmalloc_array(count, sizeof(*own), GFP_NOFS);
		if (!maps || !own) {
			kfree(maps);
			kfree(own);
			maps = NULL;
		}
	}
	for (i = 0; i < count; i++) {
		const struct fxfs_dax_extent *e = &extents[i];

		err = -EXDEV;
		if (READ_ONCE(fxfs_dax_moves) && e->extent_size && e->extent_size <= FXFS_DAX_SIZE &&
		    PAGE_ALIGNED(e->extent_offset) && (e->extent_offset || e->len < e->extent_size))
			err = fxfs_dax_move(dax, range, e, (e->offset - start) >> PAGE_SHIFT,
					    e->len >> PAGE_SHIFT);
		/*
		 * No room for the region now, nor memory to track the page there, or it can't map the
		 * piece: the range's own space does, with the others, if it gathers them.
		 */
		if (err == -EXDEV || err == -EAGAIN || err == -ENOSPC || err == -ENOMEM) {
			if (maps) {
				maps[nr_own] = (struct fxfs_device_map) {
					.branch = e->branch, .sequence = e->sequence,
					.offset = e->extent_offset, .len = e->len,
					.window_offset = range->window_offset + (e->offset - start),
					.writable = e->open,
					.ahead = true,
				};
				own[nr_own++] = i;
				continue;
			}
			err = fxfs_dax_map_own(dax, range, e);
		}
		err = fxfs_dax_mapped(range, e, err);
		if (err < 0)
			goto out;
		writable |= err;
	}
	if (nr_own) {
		many = fxfs_dax_device_map_many(dax, maps, nr_own);
		atomic64_add(nr_own, &dax->maps[FXFS_DAX_RANGE]);
		for (i = 0; i < nr_own; i++) {
			const struct fxfs_dax_extent *e = &extents[own[i]];

			atomic64_add(e->len, &dax->map_bytes[FXFS_DAX_RANGE]);
			/* If they failed, mapped again one by one, to say which, and why. */
			if (many) {
				err = fxfs_dax_map_own(dax, range, e);
			} else {
				fxfs_dax_relocate(range, (e->offset - start) >> PAGE_SHIFT,
						  e->len >> PAGE_SHIFT, NULL, 0);
				err = 0;
			}
			err = fxfs_dax_mapped(range, e, err);
			if (err < 0)
				goto out;
			writable |= err;
		}
	}
	err = 0;
out:
	if (writable)
		fxfs_dax_add_writable(dax, range->file);
	kfree(maps);
	kfree(own);
	return err;
}

/*
 * fxdax: has `range` map the pieces `extents`, where Fxfs just moved its pages, read-only in its
 * own space, outside the gate: a seal came between the move and the gate, which seals, or sealed,
 * their extents. The range mapped the pages where they were, whose blocks the move freed: once
 * their extent was freed, the device mapped zeros there, which reads returned, and a fault's copy
 * of its page took, losing the page's data (TASK-121). A slice maps its region's pages alone:
 * -EXDEV, for it to be dropped. With the file's DAX semaphore held for writing.
 */
static int fxfs_dax_map_moved(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			      struct fxfs_dax_extent *extents, ssize_t count)
{
	ssize_t i;
	int err;

	lockdep_assert_held_write(&range->file->sem);
	atomic64_inc(&dax->moved_unsealed);
	if (range->region)
		return -EXDEV;
	for (i = 0; i < count; i++) {
		struct fxfs_dax_extent *e = &extents[i];
		unsigned long page = (e->offset - fxfs_dax_chunk_start(range)) >> PAGE_SHIFT;
		unsigned long pages = e->len >> PAGE_SHIFT;

		/*
		 * A piece writable already stayed where it was, in its open extent, which the user's
		 * mappings may map writably until the seal's sealer makes them read-only: mapped
		 * read-only now, their stores would be refused. Those the sealer forgot are mapped.
		 */
		if (find_next_zero_bit(range->writable, page + pages, page) == page + pages)
			continue;
		e->open = false;
		err = fxfs_dax_map_own(dax, range, e);
		/*
		 * An extent the device doesn't hold was never written: its blocks are zeros, a hole,
		 * as fxfs_dax_mapped has them; moved from where they were, which faults replace, and
		 * no longer writable, nor mapped, as the range had them (TASK-121).
		 */
		if (err == -ENOENT) {
			fxfs_dax_relocate(range, page, pages, NULL, 0);
			fxfs_dax_clear_writable(range, page, pages);
			bitmap_clear(range->mapped, page, pages);
			continue;
		}
		err = fxfs_dax_mapped(range, e, err);
		if (err < 0)
			return err;
	}
	return 0;
}

/* Whether any of the pieces `extents` is of an open extent. */
static bool fxfs_dax_any_open(const struct fxfs_dax_extent *extents, ssize_t count)
{
	ssize_t i;

	for (i = 0; i < count; i++) {
		if (extents[i].open)
			return true;
	}
	return false;
}

/*
 * fxdax: takes the file's semaphore for reading; for a fault (`fault`), only if it is free, or
 * -EDEADLK. A fault holds its page's DAX entry, which a write(2) in the gate may wait for, while
 * the semaphore's holder, another fault, waits in Fxfs (fxfs_dax_map_write) for compaction, whose
 * seal waits for that write: the fault lets go of its entry and waits for the semaphore instead
 * (fxfs_dax_do_fault), then tries again (TASK-79.12).
 */
static int fxfs_dax_down_read(struct fxfs_dax_file *file, bool fault)
{
	if (!fault) {
		down_read(&file->sem);
		return 0;
	}
	return down_read_trylock(&file->sem) ? 0 : -EDEADLK;
}

/* fxdax: as fxfs_dax_down_read, for writing. */
static int fxfs_dax_down_write(struct fxfs_dax_file *file, bool fault)
{
	if (!fault) {
		down_write(&file->sem);
		return 0;
	}
	return down_write_trylock(&file->sem) ? 0 : -EDEADLK;
}

/*
 * Enters the device's gate (fxfs_device_store_begin), and returns the seal count: or, for a fault,
 * -EBUSY rather than wait for a seal in progress. A fault holds its page's DAX entry, which a
 * write(2) in the gate may wait for (dax_iomap_iter invalidating the page's entries), while the
 * seal waits for that write: the fault lets go of its entry and waits for the seal instead
 * (fxfs_dax_do_fault), then tries again.
 */
static long fxfs_dax_store_begin(struct fxfs_dax *dax, bool fault)
{
	u64 seals;

	if (!fault)
		return fxfs_device_store_begin(dax->dev);
	return fxfs_device_store_try_begin(dax->dev, &seals) ? (long)seals : -EBUSY;
}

/*
 * Maps the extents holding the data of the chunk `range` is for, which is unmapped: for a fault
 * (`fault`), -EBUSY rather than wait for a seal (fxfs_dax_store_begin).
 */
static int fxfs_dax_map_chunk(struct fxfs_dax *dax, struct inode *inode,
			      struct fxfs_dax_range *range, bool fault)
{
	loff_t start = fxfs_dax_chunk_start(range);
	loff_t end = min_t(loff_t, start + ((loff_t)range->pages << PAGE_SHIFT),
			   round_up(i_size_read(inode), PAGE_SIZE));
	struct fxfs_dax_extent *extents;
	ssize_t count;
	long gate;
	u64 seals;
	int err;

	if (end <= start)
		return 0;
	extents = fxfs_dax_alloc_extents(dax, DIV_ROUND_UP(end - start, PAGE_SIZE));
	if (!extents)
		return -ENOMEM;
	for (;;) {
		/*
		 * Extents open as Rust finds them are still once the gate is entered, unless a seal
		 * was in progress then, or began since.
		 */
		seals = fxfs_device_seals(dax->dev);
		count = fxfs_nofs(fxfs_dax_extents(dax->fs, inode->i_ino, start, end - start,
						   extents, DIV_ROUND_UP(end - start, PAGE_SIZE)));
		fxfs_dax_delay();
		if (count < 0) {
			err = count;
			break;
		}
		/* Sealed extents map read-only, whatever seals meanwhile, outside the gate. */
		if (!fxfs_dax_any_open(extents, count)) {
			err = fxfs_dax_map_extents(dax, range, extents, count);
			break;
		}
		gate = fxfs_dax_store_begin(dax, fault);
		if (gate < 0) {
			err = gate;
			break;
		}
		if (gate == seals) {
			err = fxfs_dax_map_extents(dax, range, extents, count);
			fxfs_device_store_end(dax->dev);
			break;
		}
		fxfs_device_store_end(dax->dev);
	}
	kvfree(extents);
	return err;
}

/* Wakes those waiting for a range to be freed, or for its last user to be done. */
static void fxfs_dax_released(struct fxfs_dax *dax)
{
	atomic_long_inc(&dax->released);
	/* Pairs the condition update with a waiter preparing to sleep before testing the queue. */
	if (wq_has_sleeper(&dax->wait))
		wake_up_all(&dax->wait);
}

/* Waits up to `ms` milliseconds for a range to be freed, or its last user done, since `seen`. */
static long fxfs_dax_wait_released(struct fxfs_dax *dax, long seen, unsigned int ms)
{
	return wait_event_killable_timeout(dax->wait, atomic_long_read(&dax->released) != seen,
					   msecs_to_jiffies(ms));
}

/*
 * `pages` pages of the window's free space: their offset in the window, or -ENOSPC if it has no
 * room. A whole chunk's range is at a chunk's alignment, if `aligned`.
 */
static long fxfs_dax_take_window(struct fxfs_dax *dax, unsigned long pages, bool aligned)
{
	size_t size = pages << PAGE_SHIFT;
	unsigned long addr;

	if (aligned && pages == FXFS_DAX_PAGES)
		addr = gen_pool_alloc_algo(dax->pool, size, gen_pool_first_fit_order_align, NULL);
	else
		addr = gen_pool_alloc(dax->pool, size);
	if (!addr)
		atomic_long_inc(&dax->window_full);
	return addr ? addr - FXFS_DAX_POOL_BASE : -ENOSPC;
}

/*
 * fxdax: `pages` pages of the window's space for regions: their offset in the window, or -EAGAIN
 * if it has no room now, as fxfs_dax_get_region says. Regions map extents of any length at any
 * offset: in the ranges' space, they left it free in pieces too small for a chunk's range, and
 * every range reclaimed others, and mapped its chunk again (TASK-91.10).
 */
static long fxfs_dax_take_region_window(struct fxfs_dax *dax, unsigned long pages)
{
	unsigned long addr = gen_pool_alloc(dax->region_pool, pages << PAGE_SHIFT);

	if (!addr)
		atomic_long_inc(&dax->regions_full);
	return addr ? addr - FXFS_DAX_POOL_BASE : -EAGAIN;
}

/* Frees `pages` pages of the window at `window_offset`, which nothing maps. */
static void fxfs_dax_give_window(struct fxfs_dax *dax, u64 window_offset, unsigned long pages)
{
	struct gen_pool *pool = window_offset < dax->regions_at ? dax->pool : dax->region_pool;

	if (pages)
		gen_pool_free(pool, window_offset + FXFS_DAX_POOL_BASE, pages << PAGE_SHIFT);
}

/*
 * A new range of `pages` pages at `window_offset`, unmapped and on no list: of its own, or a slice
 * of `region`, whose slice it takes.
 */
static struct fxfs_dax_range *fxfs_dax_new_range(u64 window_offset, unsigned long pages,
						 struct fxfs_dax_region *region)
{
	struct fxfs_dax_range *range;

	range = kzalloc(sizeof(*range), GFP_NOFS);
	if (!range)
		return ERR_PTR(-ENOMEM);
	range->mapped = bitmap_zalloc(pages, GFP_NOFS);
	range->writable = bitmap_zalloc(pages, GFP_NOFS);
	range->relocated = bitmap_zalloc(pages, GFP_NOFS);
	if (!range->mapped || !range->writable || !range->relocated) {
		bitmap_free(range->mapped);
		bitmap_free(range->writable);
		bitmap_free(range->relocated);
		kfree(range);
		return ERR_PTR(-ENOMEM);
	}
	range->window_offset = window_offset;
	range->region = region;
	range->pages = pages;
	INIT_LIST_HEAD(&range->list);
	return range;
}

static u64 fxfs_dax_region_key(u64 branch, u64 sequence)
{
	return sequence ^ (branch * GOLDEN_RATIO_64);
}

/* The region mapping the extent {branch, sequence}, if there is one. With the regions lock held. */
static struct fxfs_dax_region *fxfs_dax_find_region(struct fxfs_dax *dax, u64 branch,
						    u64 sequence)
{
	struct fxfs_dax_region *region;

	lockdep_assert_held(&dax->regions_lock);

	hash_for_each_possible(dax->regions, region, node, fxfs_dax_region_key(branch, sequence)) {
		if (region->branch == branch && region->sequence == sequence)
			return region;
	}
	return NULL;
}

/*
 * Drops a reference to `region`: with its last, the region is unmapped, and its space free again.
 */
static void fxfs_dax_put_region(struct fxfs_dax *dax, struct fxfs_dax_region *region)
{
	int err;

	spin_lock(&dax->regions_lock);
	if (--region->refs) {
		spin_unlock(&dax->regions_lock);
		return;
	}
	hash_del(&region->node);
	list_del(&region->writable_list);
	spin_unlock(&dax->regions_lock);
	if (region->mapped) {
		err = fxfs_dax_device_unmap(dax, region->window_offset,
					(u64)region->mapped << PAGE_SHIFT);
		if (err)
			pr_warn("fxfs: unmapping the DAX window at %llx failed: %d\n",
				region->window_offset, err);
	}
	fxfs_dax_give_window(dax, region->window_offset, region->pages);
	kfree(region);
	atomic_long_dec(&dax->region_count);
	fxfs_dax_released(dax);
}

/* Frees what `range` holds in memory, and it. */
static void fxfs_dax_free_range_memory(struct fxfs_dax_range *range)
{
	bitmap_free(range->mapped);
	bitmap_free(range->writable);
	bitmap_free(range->relocated);
	kfree(range->moved);
	kfree(range->regions);
	kfree(range);
}

static void fxfs_dax_free_range_rcu(struct rcu_head *rcu)
{
	fxfs_dax_free_range_memory(container_of(rcu, struct fxfs_dax_range, rcu));
}

/*
 * Frees `range`, which nothing maps, and is in no file's ranges: its space in the window is free
 * again, or its slice, and the regions its pages moved to are its no more.
 */
static void fxfs_dax_free_range(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	unsigned int i;

	spin_lock(&dax->lock);
	list_del(&range->list);
	spin_unlock(&dax->lock);
	if (range->region)
		fxfs_dax_put_region(dax, range->region);
	else
		fxfs_dax_give_window(dax, range->window_offset, range->pages);
	for (i = 0; i < range->nr_regions; i++)
		fxfs_dax_put_region(dax, range->regions[i]);
	call_rcu(&range->rcu, fxfs_dax_free_range_rcu);
	fxfs_dax_released(dax);
}

/*
 * Unmaps `range` and frees it, with its file's DAX semaphore held for writing, once nothing maps
 * its pages: its file's inode's mappings of them are gone (fxfs_dax_release), if it has one. A
 * slice's pages are its region's, which go with its last slice.
 */
static void fxfs_dax_unmap_range(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	int err;

	lockdep_assert_held_write(&range->file->sem);

	xa_erase(&range->file->ranges, range->chunk);
	if (!range->region && !bitmap_empty(range->mapped, range->pages)) {
		err = fxfs_dax_device_unmap(dax, range->window_offset,
					(u64)range->pages << PAGE_SHIFT);
		if (err)
			pr_warn("fxfs: unmapping the DAX window at %llx failed: %d\n",
				range->window_offset, err);
	}
	fxfs_dax_free_range(dax, range);
}

/* Drops a reference to `file`, freeing it once it has none. */
static void fxfs_dax_put_file(struct fxfs_dax *dax, struct fxfs_dax_file *file)
{
	if (!refcount_dec_and_test(&file->refs))
		return;
	mutex_lock(&dax->writable_lock);
	list_del_init(&file->writable);
	mutex_unlock(&dax->writable_lock);
	xa_destroy(&file->ranges);
	kfree(file);
}

/*
 * Forgets `file` if no inode is read in for it and it has no ranges, with the files lock held:
 * returns whether it did, when the caller drops the filesystem's reference to it, once it has
 * unlocked the files lock (fxfs_dax_put_file takes the writable lock, which the sealer takes
 * before it).
 */
static bool fxfs_dax_forget_file(struct fxfs_dax *dax, struct fxfs_dax_file *file)
{
	lockdep_assert_held(&dax->files_lock);
	if (file->inode || !xa_empty(&file->ranges) || xa_load(&dax->files, file->ino) != file)
		return false;
	xa_erase(&dax->files, file->ino);
	return true;
}

/*
 * Removes the inode's mappings of the chunk `range` maps, and the DAX entries for them, unless
 * a page is in use (pinned): with the inode's invalidate_lock held for writing.
 */
static bool fxfs_dax_release(struct inode *inode, struct fxfs_dax_range *range)
{
	struct address_space *mapping = inode->i_mapping;
	loff_t start = fxfs_dax_chunk_start(range);
	loff_t end = start + ((loff_t)range->pages << PAGE_SHIFT) - 1;

	if (dax_layout_busy_page_range(mapping, start, end))
		return false;
	/* Dirty entries go only once written back, which only cleans them. */
	if (filemap_fdatawrite_range(mapping, start, end))
		return false;
	return !invalidate_inode_pages2_range(mapping, start >> PAGE_SHIFT, end >> PAGE_SHIFT);
}

/*
 * Reclaims `file`'s range of chunk `chunk`, if no iomap uses it and its file's locks are free,
 * and returns whether it did. A file with no inode has nothing mapping its ranges but the window.
 */
static bool fxfs_dax_reclaim_range(struct fxfs_dax *dax, struct fxfs_dax_file *file,
				   pgoff_t chunk)
{
	struct fxfs_dax_range *range;
	struct inode *inode = NULL;
	bool reclaimed = false, forgotten = false;

	mutex_lock(&dax->files_lock);
	if (file->inode)
		inode = igrab(file->inode);
	mutex_unlock(&dax->files_lock);

	if (inode) {
		if (down_write_trylock(&inode->i_mapping->invalidate_lock)) {
			if (down_write_trylock(&file->sem)) {
				range = xa_load(&file->ranges, chunk);
				if (range && !atomic_read(&range->users) &&
				    fxfs_dax_release(inode, range)) {
					fxfs_dax_unmap_range(dax, range);
					reclaimed = true;
				}
				up_write(&file->sem);
			}
			filemap_invalidate_unlock(inode->i_mapping);
		}
		iput(inode);
		return reclaimed;
	}
	if (!down_write_trylock(&file->sem))
		return false;
	mutex_lock(&dax->files_lock);
	/* An inode read in since may map the range: it is reclaimed with it next time. */
	if (!file->inode) {
		range = xa_load(&file->ranges, chunk);
		if (range && !atomic_read(&range->users)) {
			fxfs_dax_unmap_range(dax, range);
			reclaimed = true;
		}
		forgotten = fxfs_dax_forget_file(dax, file);
	}
	mutex_unlock(&dax->files_lock);
	up_write(&file->sem);
	if (forgotten)
		fxfs_dax_put_file(dax, file);
	return reclaimed;
}

/* Reclaims the least recently used range no iomap uses, and returns whether it did. */
static bool fxfs_dax_reclaim(struct fxfs_dax *dax)
{
	struct fxfs_dax_file *file = NULL;
	struct fxfs_dax_range *range;
	pgoff_t chunk = 0;
	bool reclaimed;

	spin_lock(&dax->lock);
	list_for_each_entry(range, &dax->used, list) {
		if (atomic_read(&range->users))
			continue;
		/* Tried once: the next try starts with another. */
		list_move_tail(&range->list, &dax->used);
		file = range->file;
		/* Kept while it is tried: a range on the list keeps its file. */
		refcount_inc(&file->refs);
		chunk = range->chunk;
		break;
	}
	spin_unlock(&dax->lock);
	if (!file)
		return false;
	reclaimed = fxfs_dax_reclaim_range(dax, file, chunk);
	fxfs_dax_put_file(dax, file);
	if (reclaimed)
		atomic_long_inc(&dax->reclaims);
	return reclaimed;
}

/*
 * `pages` pages of the window, as fxfs_dax_take_window, reclaimed if it has no room: their offset.
 * Waits for room, unless `nowait` asks, as a fault does, which can't: -EAGAIN then.
 */
static long fxfs_dax_alloc_window(struct fxfs_dax *dax, unsigned long pages, bool aligned,
				  bool nowait)
{
	long window_offset, seen;

	for (;;) {
		seen = atomic_long_read(&dax->released);
		window_offset = fxfs_dax_take_window(dax, pages, aligned);
		if (window_offset >= 0)
			return window_offset;
		if (fxfs_dax_reclaim(dax))
			continue;
		if (nowait)
			return -EAGAIN;
		/* Ranges in use, or whose inodes are locked, free up soon. */
		if (fxfs_dax_wait_released(dax, seen, 10) < 0)
			return -EINTR;
	}
}

/* A new range of its own of `pages` pages, as fxfs_dax_alloc_window finds room for it. */
static struct fxfs_dax_range *fxfs_dax_alloc_range(struct fxfs_dax *dax, unsigned long pages,
						   bool fault)
{
	long window_offset = fxfs_dax_alloc_window(dax, pages, true, fault);
	struct fxfs_dax_range *range;

	if (window_offset < 0)
		return ERR_PTR(window_offset);
	range = fxfs_dax_new_range(window_offset, pages, NULL);
	if (IS_ERR(range))
		fxfs_dax_give_window(dax, window_offset, pages);
	return range;
}

/*
 * The region mapping extent `e`'s, made, unmapped, if there is none: with a reference more, the
 * caller's. -EAGAIN if the window has no room for it now, reclaiming ranges for it if `reclaim`
 * says and regions share the ranges' space: the caller holds a file's DAX semaphore, and can't
 * wait for room. fxdax: in a space of their own, regions reclaim nothing: their callers map what
 * they would have in ranges' (TASK-91.10).
 */
static struct fxfs_dax_region *fxfs_dax_get_region(struct fxfs_dax *dax,
						   const struct fxfs_dax_extent *e, bool reclaim)
{
	unsigned long pages = DIV_ROUND_UP(e->extent_size, PAGE_SIZE);
	struct fxfs_dax_region *region, *new;
	long window_offset;

	spin_lock(&dax->regions_lock);
	region = fxfs_dax_find_region(dax, e->branch, e->sequence);
	if (region)
		region->refs++;
	spin_unlock(&dax->regions_lock);
	if (region)
		return region;

	new = kzalloc(sizeof(*new), GFP_NOFS);
	if (!new)
		return ERR_PTR(-ENOMEM);
	/* A region maps an extent at any offset: its space needs no alignment. */
	if (dax->region_pool != dax->pool)
		window_offset = fxfs_dax_take_region_window(dax, pages);
	else
		window_offset = reclaim ? fxfs_dax_alloc_window(dax, pages, false, true) :
					  fxfs_dax_take_window(dax, pages, false);
	if (window_offset < 0) {
		kfree(new);
		return ERR_PTR(window_offset);
	}
	new->branch = e->branch;
	new->sequence = e->sequence;
	new->window_offset = window_offset;
	new->pages = pages;
	new->refs = 1;
	mutex_init(&new->lock);
	INIT_LIST_HEAD(&new->writable_list);
	new->dax = dax;
	INIT_WORK(&new->grow, fxfs_dax_grow);
	atomic_set(&new->growing, 0);
	atomic_long_set(&new->reached, 0);
	spin_lock(&dax->regions_lock);
	/* Another may have made it meanwhile. */
	region = fxfs_dax_find_region(dax, e->branch, e->sequence);
	if (region)
		region->refs++;
	else
		hash_add(dax->regions, &new->node, fxfs_dax_region_key(e->branch, e->sequence));
	spin_unlock(&dax->regions_lock);
	if (!region) {
		atomic_long_inc(&dax->region_count);
		return new;
	}
	fxfs_dax_give_window(dax, window_offset, pages);
	kfree(new);
	return region;
}

/*
 * Makes `region`, of an open extent, writable, unless it is mapped already. In the gate, where only
 * writable regions are mapped further. fxdax: under the region's lock, which a MAP of it holds: a
 * read-only one in flight, outside the gate, by a reader that found the extent sealed, ended after
 * this had made the region writable and claimed its pages mapped, read-only, where writers took
 * them for writable: the host refused their stores, and the VM stopped (TASK-99).
 */
static void fxfs_dax_region_open(struct fxfs_dax *dax, struct fxfs_dax_region *region)
{
	fxfs_device_assert_in_gate(dax->dev);
	fxfs_dax_delay();
	mutex_lock(&region->lock);
	spin_lock(&dax->regions_lock);
	if (!region->writable && !region->mapped) {
		region->writable = true;
		list_add_tail(&region->writable_list, &dax->writable_regions);
	}
	spin_unlock(&dax->regions_lock);
	mutex_unlock(&region->lock);
}

/*
 * fxdax: warns unless pages up to `to` of `region`, which a MAP mapped, are still the region's: a
 * seal shrinks a writable region to what it maps, giving the rest of its space back, which the
 * next region may take at once (fxfs_dax_restage_work); a MAP past that maps another region's
 * window pages, whose stores the host then refuses.
 */
static void fxfs_dax_assert_mapped_within(struct fxfs_dax *dax, struct fxfs_dax_region *region,
					  unsigned long to, bool writable, enum fxfs_dax_why why)
{
	unsigned long pages;

	spin_lock(&dax->regions_lock);
	pages = region->pages;
	spin_unlock(&dax->regions_lock);
	WARN(to > pages,
	     "fxfs: mapped pages to %lu of extent %llx:%llu, %s (%s), past its region's %lu pages at %llu of the DAX window\n",
	     to, region->branch, region->sequence, writable ? "writably" : "read-only",
	     fxfs_dax_whys[why], pages, region->window_offset);
}

/*
 * Maps `region`'s extent as far as page `end` at least, in the gate, where the device has sealed
 * the extents the filesystem has: read-only, all of it if it can; or, while the extent is open,
 * writably, and further, doubling what is mapped by up to FXFS_DAX_REGION_STEP pages, from
 * FXFS_DAX_REGION_FIRST, so that the next blocks it hands out need no MAP. -EXDEV if the region's
 * space doesn't reach so far: sealed, it shrinks to what it mapped.
 */
static int fxfs_dax_map_to(struct fxfs_dax *dax, struct fxfs_dax_region *region, unsigned long end,
			   enum fxfs_dax_why why)
{
	unsigned long from, to;
	/*
	 * As the region's lock, held, keeps it: fxfs_dax_region_open sets it under that lock, and a
	 * seal clears it with no store in progress, so during no writable MAP.
	 */
	bool writable;
	int err = 0;

	mutex_lock(&region->lock);
	writable = READ_ONCE(region->writable);
	/* A read-only MAP may be a reader's, outside the gate; a writable one is in it. */
	if (writable)
		fxfs_device_assert_in_gate(dax->dev);
	from = region->mapped;
	if (end <= from)
		goto out;
	if (end > region->pages) {
		err = -EXDEV;
		goto out;
	}
	to = region->pages;
	if (writable)
		to = min(to, max(end, from + clamp(from, FXFS_DAX_REGION_FIRST, FXFS_DAX_REGION_STEP)));
	for (;;) {
		err = fxfs_dax_device_map(dax, region->branch, region->sequence,
				      (u64)from << PAGE_SHIFT, (u64)(to - from) << PAGE_SHIFT,
				      region->window_offset + ((u64)from << PAGE_SHIFT), writable,
				      why == FXFS_DAX_REACH);
		fxfs_dax_delay();
		atomic64_inc(&dax->maps[why]);
		atomic64_add((u64)(to - from) << PAGE_SHIFT, &dax->map_bytes[why]);
		/* A sealed extent may end before what was allocated of it: past what was written. */
		if (!err || writable || to == end)
			break;
		to = end;
	}
	/*
	 * An extent the device doesn't hold (-ENOENT) maps nothing: its callers read its pages as
	 * holes, which the region must not claim as mapped, or a slice of it would hand the guest
	 * window pages the host never mapped.
	 */
	if (!err) {
		fxfs_dax_assert_mapped_within(dax, region, to, writable, why);
		WRITE_ONCE(region->mapped, to);
	} else if (err != -ENOENT)
		pr_warn("fxfs: mapping pages %lu to %lu of extent %llx:%llu, of %lu pages, %s, at %llu of the DAX window failed: %d\n",
			from, to, region->branch, region->sequence, region->pages,
			writable ? "writably" : "read-only", region->window_offset, err);
out:
	mutex_unlock(&region->lock);
	return err;
}

/*
 * Has `region`'s writable extent mapped a step further in the background, unless it is mapped
 * far enough ahead of the writes already: once they reach the second half of what it maps, the
 * next step, so that they rarely wait for a MAP (a MAP of a step takes most of a millisecond).
 */
static void fxfs_dax_grow_ahead(struct fxfs_dax *dax, struct fxfs_dax_region *region)
{
	unsigned long mapped = READ_ONCE(region->mapped);
	unsigned long reached = atomic_long_read(&region->reached);

	if (!READ_ONCE(region->writable) || mapped >= region->pages ||
	    reached + clamp(mapped, FXFS_DAX_REGION_FIRST, FXFS_DAX_REGION_STEP) / 2 <= mapped)
		return;
	if (atomic_cmpxchg(&region->growing, 0, 1))
		return;
	spin_lock(&dax->regions_lock);
	region->refs++;
	spin_unlock(&dax->regions_lock);
	atomic_inc(&dax->growing);
	queue_work(dax->grow_wq, &region->grow);
}

/*
 * fxdax: maps `region`'s writable extent one step further, in the gate (unless it is sealed by
 * then), and again if the writes went on meanwhile; and drops the reference its queuing took.
 */
static void fxfs_dax_grow(struct work_struct *work)
{
	struct fxfs_dax_region *region = container_of(work, struct fxfs_dax_region, grow);
	struct fxfs_dax *dax = region->dax;
	unsigned long mapped;

	fxfs_device_store_begin(dax->dev);
	/* In the gate, no seal makes it read-only, or shrinks it. */
	mapped = READ_ONCE(region->mapped);
	if (READ_ONCE(region->writable) && mapped < region->pages)
		fxfs_dax_map_to(dax, region, mapped + 1, FXFS_DAX_GROW);
	fxfs_device_store_end(dax->dev);
	atomic_set(&region->growing, 0);
	fxfs_dax_grow_ahead(dax, region);
	fxfs_dax_put_region(dax, region);
	atomic_dec(&dax->growing);
}

/*
 * Maps `region`'s extent as far as page `end` at least (fxfs_dax_map_to), in the gate; and a
 * writable region's further, in the background, ahead of the writes (fxfs_dax_grow_ahead).
 */
static int fxfs_dax_reach(struct fxfs_dax *dax, struct fxfs_dax_region *region, unsigned long end)
{
	unsigned long reached;
	int err;

	err = fxfs_dax_map_to(dax, region, end, FXFS_DAX_REACH);
	if (err || !READ_ONCE(region->writable))
		return err;
	reached = atomic_long_read(&region->reached);
	while (reached < end && !atomic_long_try_cmpxchg(&region->reached, &reached, end))
		;
	fxfs_dax_grow_ahead(dax, region);
	return 0;
}

/*
 * Keeps a reference to `region`, whose reference the caller passes, in `range`'s regions, unless it
 * has one: the caller's goes then.
 */
static int fxfs_dax_keep_region(struct fxfs_dax *dax, struct fxfs_dax_range *range,
				struct fxfs_dax_region *region)
{
	struct fxfs_dax_region **regions;
	unsigned int i;

	for (i = 0; i < range->nr_regions; i++) {
		if (range->regions[i] == region) {
			fxfs_dax_put_region(dax, region);
			return 0;
		}
	}
	/* Room for 4, then twice as many as it has as it fills. */
	if (!range->nr_regions || (range->nr_regions >= 4 && is_power_of_2(range->nr_regions))) {
		regions = krealloc_array(range->regions, max(4U, 2 * range->nr_regions),
					 sizeof(*regions), GFP_NOFS);
		if (!regions) {
			fxfs_dax_put_region(dax, region);
			return -ENOMEM;
		}
		range->regions = regions;
	}
	range->regions[range->nr_regions++] = region;
	return 0;
}

/*
 * fxdax: maps the piece `e`, pages `page` on of the chunk the range of its own `range` maps, of an
 * extent no larger than a chunk, where the extent's region maps it, rather than in the range's own
 * space (the region made if there is none, and mapped as far as the piece, writably if it is
 * open): so that the window maps the extent once, and blocks moved to the staging extent take no
 * MAP. -ENOSPC if the window has no room for the region now, -EXDEV if the region can't map the
 * piece writably, as it is sealed. In the gate if `e` is open, with the file's DAX semaphore held
 * for writing.
 */
static int fxfs_dax_move(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			 const struct fxfs_dax_extent *e, unsigned long page, unsigned long pages)
{
	unsigned long first = e->extent_offset >> PAGE_SHIFT;
	struct fxfs_dax_region *region;
	int err;

	lockdep_assert_held_write(&range->file->sem);

	if (!range->moved) {
		range->moved = kcalloc(range->pages, sizeof(*range->moved), GFP_NOFS);
		if (!range->moved)
			return -ENOMEM;
	}
	/* Not reclaiming other ranges for it, in the gate: the range's own space maps it then. */
	region = fxfs_dax_get_region(dax, e, false);
	if (IS_ERR(region))
		return PTR_ERR(region);
	if (e->open) {
		fxfs_dax_region_open(dax, region);
		if (!region->writable) {
			fxfs_dax_put_region(dax, region);
			return -EXDEV;
		}
	}
	err = fxfs_dax_reach(dax, region, first + pages);
	if (err) {
		fxfs_dax_put_region(dax, region);
		return err;
	}
	err = fxfs_dax_keep_region(dax, range, region);
	if (err)
		return err;
	fxfs_dax_relocate(range, page, pages, region, first);
	return 0;
}

/*
 * Makes `inode`'s chunk `chunk` a slice of its extent's region, if its data in the file's `size`
 * bytes is one piece of an extent the window maps whole, and returns it, kept, in its file's
 * ranges: writable if the extent is open. NULL if it isn't such a piece, or the window has no room
 * for its region now; for a fault (`fault`), -EBUSY rather than wait for a seal. With its file's
 * DAX semaphore held for writing.
 */
static struct fxfs_dax_range *fxfs_dax_new_slice(struct fxfs_dax *dax, struct inode *inode,
						 pgoff_t chunk, loff_t size, bool fault)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	loff_t start = (loff_t)chunk << FXFS_DAX_SHIFT;
	loff_t end = min_t(loff_t, start + FXFS_DAX_SIZE, round_up(size, PAGE_SIZE));
	struct fxfs_dax_region *region;
	struct fxfs_dax_range *range;
	struct fxfs_dax_extent e;
	unsigned long first, pages;
	ssize_t count;
	long gate;
	u64 seals;
	int err;

	lockdep_assert_held_write(&file->sem);

	if (end <= start)
		return NULL;
	pages = (end - start) >> PAGE_SHIFT;
	for (;;) {
		seals = fxfs_device_seals(dax->dev);
		count = fxfs_nofs(fxfs_dax_extents(dax->fs, inode->i_ino, start, end - start, &e, 1));
		fxfs_dax_delay();
		/* Holes, or more than one piece. */
		if (count == 0 || count == -E2BIG)
			return NULL;
		if (count < 0)
			return ERR_PTR(count);
		if (e.offset != start || e.len != end - start || !e.extent_size ||
		    e.extent_size > FXFS_DAX_SIZE || !PAGE_ALIGNED(e.extent_offset))
			return NULL;
		region = fxfs_dax_get_region(dax, &e, true);
		if (IS_ERR(region))
			return PTR_ERR(region) == -EAGAIN ? NULL : ERR_CAST(region);
		first = e.extent_offset >> PAGE_SHIFT;
		/* A sealed extent maps read-only, whatever seals meanwhile, outside the gate. */
		if (!e.open)
			break;
		/* As in fxfs_dax_map_chunk: open still, unless a seal was in progress, or began since. */
		gate = fxfs_dax_store_begin(dax, fault);
		if (gate < 0) {
			fxfs_dax_put_region(dax, region);
			return ERR_PTR(gate);
		}
		if (gate == seals) {
			fxfs_dax_region_open(dax, region);
			break;
		}
		fxfs_device_store_end(dax->dev);
		fxfs_dax_put_region(dax, region);
	}
	err = fxfs_dax_reach(dax, region, first + pages);
	if (err)
		goto out;
	range = fxfs_dax_new_range(region->window_offset + ((u64)first << PAGE_SHIFT), pages,
				   region);
	if (IS_ERR(range)) {
		err = PTR_ERR(range);
		goto out;
	}
	range->file = file;
	range->chunk = chunk;
	atomic_set(&range->users, 1);
	bitmap_set(range->mapped, 0, pages);
	err = xa_err(xa_store(&file->ranges, chunk, range, GFP_NOFS));
	if (err) {
		/* Its slice goes with it. */
		region = NULL;
		fxfs_dax_free_range(dax, range);
		goto out;
	}
	if (e.open && region->writable) {
		bitmap_set(range->writable, 0, pages);
		fxfs_dax_add_writable(dax, file);
	}
	spin_lock(&dax->lock);
	list_add_tail(&range->list, &dax->used);
	spin_unlock(&dax->lock);
	if (e.open)
		fxfs_device_store_end(dax->dev);
	return range;
out:
	if (e.open)
		fxfs_device_store_end(dax->dev);
	if (region)
		fxfs_dax_put_region(dax, region);
	/*
	 * Its region can't reach it, or its extent was never written: a range of its own maps it,
	 * or the holes it is.
	 */
	return err == -EXDEV || err == -ENOENT ? NULL : ERR_PTR(err);
}

/*
 * Makes room for a fault that found none, holding the lock it held: reclaims a range, which it can
 * now, if one of its own inode's, or waits a moment for one released since `seen`.
 */
static void fxfs_dax_make_room(struct fxfs_dax *dax, long seen)
{
	if (!fxfs_dax_reclaim(dax))
		fxfs_dax_wait_released(dax, seen, 10);
}

/* How many pages the range of chunk `chunk` of a file `size` bytes long takes. */
static unsigned long fxfs_dax_range_pages(pgoff_t chunk, loff_t size)
{
	loff_t start = (loff_t)chunk << FXFS_DAX_SHIFT;

	if (size <= start)
		return 1;
	return min_t(unsigned long, FXFS_DAX_PAGES, DIV_ROUND_UP(size - start, PAGE_SIZE));
}

/*
 * How many pages a range of its own for chunk `chunk` of a file `size` bytes long takes: what the
 * file needs, rounded up to a power of two, so that a file growing a page at a time, as a log
 * appended to does, outgrows its range, and maps its data again, only as often as its size
 * doubles.
 */
static unsigned long fxfs_dax_range_capacity(pgoff_t chunk, loff_t size)
{
	return min_t(unsigned long, FXFS_DAX_PAGES,
		     roundup_pow_of_two(fxfs_dax_range_pages(chunk, size)));
}

static void fxfs_dax_use_range(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	/* Its user keeps it linked and alive. Already last, this access cannot make it newer. */
	if (READ_ONCE(range->list.next) == &dax->used)
		return;
	spin_lock(&dax->lock);
	list_move_tail(&range->list, &dax->used);
	spin_unlock(&dax->lock);
}

/*
 * The range mapping `inode`'s chunk `chunk`, mapped first if it isn't, for a file at least `size`
 * bytes long, kept for an iomap: a slice of its extent's region if it can be, or a range of its
 * own. For a fault (`fault`), -EBUSY rather than wait for a seal (fxfs_dax_store_begin), and
 * fxdax: -EDEADLK rather than for the file's semaphore (fxfs_dax_down_read).
 */
static struct fxfs_dax_range *fxfs_dax_get_range(struct inode *inode, pgoff_t chunk, loff_t size,
						 bool fault)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range, *new;
	unsigned long pages;
	int err;

	err = fxfs_dax_down_read(file, fault);
	if (err)
		return ERR_PTR(err);
	range = xa_load(&file->ranges, chunk);
	if (range)
		atomic_inc(&range->users);
	up_read(&file->sem);
	if (range)
		goto used;

	err = fxfs_dax_down_write(file, fault);
	if (err)
		return ERR_PTR(err);
	range = xa_load(&file->ranges, chunk);
	if (range)
		atomic_inc(&range->users);
	else
		range = fxfs_dax_new_slice(dax, inode, chunk, max(i_size_read(inode), size), fault);
	up_write(&file->sem);
	if (IS_ERR(range))
		return range;
	if (range)
		goto used;

	pages = fxfs_dax_range_capacity(chunk, max(i_size_read(inode), size));
	new = fxfs_dax_alloc_range(dax, pages, fault);
	if (IS_ERR(new))
		return new;
	err = fxfs_dax_down_write(file, fault);
	if (err) {
		fxfs_dax_free_range(dax, new);
		return ERR_PTR(err);
	}
	/* Another iomap may have mapped it meanwhile. */
	range = xa_load(&file->ranges, chunk);
	if (range) {
		atomic_inc(&range->users);
		up_write(&file->sem);
		fxfs_dax_free_range(dax, new);
		goto used;
	}
	new->file = file;
	new->chunk = chunk;
	atomic_set(&new->users, 1);
	err = xa_err(xa_store(&file->ranges, chunk, new, GFP_NOFS));
	if (!err) {
		spin_lock(&dax->lock);
		list_add_tail(&new->list, &dax->used);
		spin_unlock(&dax->lock);
		err = fxfs_dax_map_chunk(dax, inode, new, fault);
		if (err)
			fxfs_dax_unmap_range(dax, new);
	} else {
		fxfs_dax_free_range(dax, new);
	}
	up_write(&file->sem);
	if (err)
		return ERR_PTR(err);
	range = new;
used:
	fxfs_dax_use_range(dax, range);
	return range;
}

static void fxfs_dax_put_range(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	if (atomic_dec_and_test(&range->users))
		fxfs_dax_released(dax);
}

/*
 * Drops `inode`'s ranges of chunks `first` to `last` too small for the file to be `size` bytes
 * long, or all of them if `size` is negative, once nothing uses their pages: they are mapped again
 * as needed. With the inode's invalidate_lock held for writing. fxdax: unless `pinned`, -EBUSY
 * rather than wait for a page pinned (TASK-79.49).
 */
static int fxfs_dax_drop_pinned(struct inode *inode, pgoff_t first, pgoff_t last, loff_t size,
				bool pinned)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range;
	unsigned long chunk;
	int err;

	for (;;) {
		long seen = atomic_long_read(&dax->released);
		bool busy = false;

		down_write(&file->sem);
		xa_for_each_range(&file->ranges, chunk, range, first, last) {
			if (size >= 0 && range->pages >= fxfs_dax_range_pages(chunk, size))
				continue;
			if (atomic_read(&range->users) || !fxfs_dax_release(inode, range)) {
				busy = true;
				break;
			}
			fxfs_dax_unmap_range(dax, range);
		}
		up_write(&file->sem);
		if (!busy)
			return 0;
		/* fxdax: a page pinned, which a fault doesn't wait for (fxfs_dax_drop_slice). */
		if (!pinned && dax_layout_busy_page_range(inode->i_mapping,
							   (loff_t)first << FXFS_DAX_SHIFT,
							   ((loff_t)(last + 1) << FXFS_DAX_SHIFT) - 1))
			return -EBUSY;
		/* A fault in progress, or a page pinned: wait for it. */
		err = fxfs_dax_break_layouts(inode, (loff_t)first << FXFS_DAX_SHIFT);
		if (err)
			return err;
		if (fxfs_dax_wait_released(dax, seen, 10) < 0)
			return -EINTR;
	}
}

/* fxfs_dax_drop_pinned, waiting for pages pinned. */
static int fxfs_dax_drop(struct inode *inode, pgoff_t first, pgoff_t last, loff_t size)
{
	return fxfs_dax_drop_pinned(inode, first, last, size, true);
}

/*
 * Drops the ranges of `inode`'s file too small for it to be `size` bytes long: those of the chunks
 * its end moves past or into, which are mapped again as large as they need. With the inode's lock
 * and invalidate_lock held for writing.
 */
static int fxfs_dax_fit(struct inode *inode, loff_t size)
{
	loff_t old = i_size_read(inode);

	lockdep_assert_held_write(&inode->i_mapping->invalidate_lock);

	if (size <= old)
		return 0;
	return fxfs_dax_drop(inode, old >> FXFS_DAX_SHIFT, (size - 1) >> FXFS_DAX_SHIFT, size);
}

/*
 * Records appends already stored through writable pages, before Fxfs reads or changes their
 * blocks using its size (TASK-92.28). With the file's semaphore held for writing.
 */
static int fxfs_dax_write_size_locked(struct inode *inode)
{
	return fxfs_nofs(fxfs_dax_update_size(fxfs_vfs_fs(inode->i_sb), inode->i_ino,
					   i_size_read(inode)));
}

/*
 * Inode writeback takes no i_rwsem: the caller may hold it while waiting for writeback. The
 * invalidate_lock excludes truncate and reflink, and the file's semaphore excludes extending
 * iomaps, from reading the size until committing it. No old writeback can undo a later truncate.
 */
int fxfs_dax_write_size(struct inode *inode)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	int err;

	if (!IS_DAX(inode))
		return 0;
	filemap_invalidate_lock_shared(inode->i_mapping);
	down_write(&file->sem);
	err = fxfs_dax_write_size_locked(inode);
	up_write(&file->sem);
	filemap_invalidate_unlock_shared(inode->i_mapping);
	return err;
}

/*
 * Drops `inode`'s range of the chunk at `pos`, a slice a write can't use (-EXDEV), to be mapped
 * again where its data now is. fxdax: for a fault (`fault`), -EBUSY rather than wait for pages
 * pinned, which the call faulting may hold itself (TASK-79.49).
 */
static int fxfs_dax_drop_slice(struct inode *inode, loff_t pos, bool fault)
{
	int err;

	filemap_invalidate_lock(inode->i_mapping);
	err = fxfs_dax_drop_pinned(inode, pos >> FXFS_DAX_SHIFT, pos >> FXFS_DAX_SHIFT, -1, !fault);
	filemap_invalidate_unlock(inode->i_mapping);
	return err;
}

/*
 * Makes room through `reserved`, recording only `size`, with an exact-write fallback. fxdax: the
 * blocks hold `held`, if not NULL, which is `reserved - pos` bytes (fxfs_dax_map_write).
 */
static ssize_t fxfs_dax_reserve_write(struct inode *inode, loff_t pos, loff_t end,
				    loff_t reserved, loff_t size, bool keep, const void *held,
				    struct fxfs_dax_extent *extents, size_t capacity)
{
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	ssize_t count;

	atomic64_inc(&dax->map_writes);
	count = fxfs_nofs(fxfs_dax_map_write(dax->fs, inode->i_ino, pos, reserved - pos,
					   size, keep, held, extents, capacity));
	/* Speculation must not refuse a write that its own bytes have room for. */
	if (!held && reserved > end &&
	    (count == -ENOSPC || count == -EDQUOT || count == -ENOMEM)) {
		atomic64_inc(&dax->map_writes);
		count = fxfs_nofs(fxfs_dax_map_write(dax->fs, inode->i_ino, pos, end - pos,
						   size, keep, NULL, extents, capacity));
	}
	return count;
}

/*
 * fxdax: after two pages appended with no seal between, reserve the rest of a small aligned
 * run. Its new blocks are zeros beyond EOF; only the write's actual end becomes the file's size.
 * One-off writes and writes after every barrier keep their former allocation size (TASK-92.58).
 */
static loff_t fxfs_dax_append_end(struct inode *inode, loff_t pos, loff_t end)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	loff_t base;

	if (pos != i_size_read(inode) || file->append_next != pos ||
	    file->append_bytes < 2 * PAGE_SIZE ||
	    file->append_seals != fxfs_device_seals(fxfs_dax_of(inode)->dev))
		return end;
	base = round_down(end - 1, FXFS_DAX_APPEND_SIZE);
	if (base > inode->i_sb->s_maxbytes - (loff_t)FXFS_DAX_APPEND_SIZE)
		return end;
	return base + FXFS_DAX_APPEND_SIZE;
}

/* Makes room before mapping a chunk's range for the first time. */
static int fxfs_dax_prepare_write(struct inode *inode, loff_t pos, loff_t len, loff_t size,
				  loff_t reserved, bool fault, struct fxfs_dax_extent *extents,
				  size_t capacity)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	ssize_t count;

	count = fxfs_dax_down_write(file, fault);
	if (count)
		return count;
	if (!xa_load(&file->ranges, pos >> FXFS_DAX_SHIFT)) {
		count = fxfs_dax_write_size_locked(inode);
		if (count)
			goto out;
		count = fxfs_dax_reserve_write(inode, pos, pos + len, max(pos + len, reserved),
					     size, fault, NULL, extents, capacity);
		if (count >= 0 && size > i_size_read(inode))
			i_size_write(inode, size);
	}
out:
	up_write(&file->sem);
	return count < 0 ? count : 0;
}

/* Fills in `iomap` for a hole of `length` bytes. */
static void fxfs_dax_hole(struct iomap *iomap, loff_t length)
{
	iomap->type = IOMAP_HOLE;
	iomap->addr = IOMAP_NULL_ADDR;
	iomap->length = length;
}

/*
 * Maps a read of up to `length` bytes at `pos`: a run of mapped pages in a row in the window, or of
 * holes. A fault's iomap of pages relocated since says they are shared (fxfs_dax_cow).
 */
static int fxfs_dax_iomap_read(struct inode *inode, loff_t pos, loff_t length, unsigned int flags,
			       struct iomap *iomap, struct iomap *srcmap)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range;
	unsigned long page, end_page;
	bool fault = flags & IOMAP_FAULT;
	loff_t start;
	bool mapped;
	u64 at = 0;
	int err;

	if (pos >= i_size_read(inode)) {
		fxfs_dax_hole(iomap, length);
		return 0;
	}
	/* A cache hit keeps the lookup's read lock through its iomap, rather than taking it twice. */
	err = fxfs_dax_down_read(file, fault);
	if (err)
		return err;
	range = xa_load(&file->ranges, pos >> FXFS_DAX_SHIFT);
	if (range) {
		atomic_inc(&range->users);
		fxfs_dax_use_range(dax, range);
	} else {
		up_read(&file->sem);
		range = fxfs_dax_get_range(inode, pos >> FXFS_DAX_SHIFT, 0, fault);
		if (IS_ERR(range))
			return PTR_ERR(range);
		err = fxfs_dax_down_read(file, fault);
		if (err) {
			fxfs_dax_put_range(dax, range);
			return err;
		}
	}
	/* fxdax: the window between a follower's extents read and its mapping, widened in tests. */
	if (fxfs_vfs_follows(inode->i_sb) && fxfs_follow_iomap_delay_us) {
		up_read(&file->sem);
		fsleep(fxfs_follow_iomap_delay_us);
		err = fxfs_dax_down_read(file, fault);
		if (err) {
			fxfs_dax_put_range(dax, range);
			return err;
		}
	}
	start = fxfs_dax_chunk_start(range);
	page = (pos - start) >> PAGE_SHIFT;
	/* A range covers the file's chunk as its size grows (fxfs_dax_fit). */
	if (WARN_ON_ONCE(page >= range->pages)) {
		up_read(&file->sem);
		fxfs_dax_put_range(dax, range);
		return -EIO;
	}
	mapped = test_bit(page, range->mapped);
	end_page = mapped ? find_next_zero_bit(range->mapped, range->pages, page) :
			    find_next_bit(range->mapped, range->pages, page);
	if (mapped) {
		at = fxfs_dax_page_offset(range, page);
		/* As far as the read goes, at most: the run needn't be found further. */
		end_page = min_t(unsigned long, end_page,
				 page + DIV_ROUND_UP(offset_in_page(pos) + length, PAGE_SIZE));
		end_page = page + fxfs_dax_run(range, page, end_page);
	}
	length = min_t(loff_t, length, start + ((loff_t)end_page << PAGE_SHIFT) - pos);
	if (!mapped) {
		up_read(&file->sem);
		fxfs_dax_put_range(dax, range);
		fxfs_dax_hole(iomap, length);
		return 0;
	}
	iomap->type = IOMAP_MAPPED;
	iomap->addr = at + offset_in_page(pos);
	iomap->length = length;
	iomap->private = range;
	/* fxdax: a PMD's, its pages in a row, which a page moving breaks up (TASK-123). */
	if ((flags & IOMAP_FAULT) && length >= PMD_SIZE)
		WRITE_ONCE(range->huge, true);
	/* A read's copy needs no mapping of the pages replaced. */
	if (flags & IOMAP_FAULT)
		fxfs_dax_cow(inode, range, page, end_page - page, flags, iomap, srcmap);
	up_read(&file->sem);
	return 0;
}

/*
 * The guest's memory of the window's page at `window_offset`. fxdax: with dax_read_lock held, as
 * dax_direct_access needs, for as long as the caller uses the memory; a debug kernel's lockdep
 * found faults taking it without (TASK-44.8).
 */
static void *fxfs_dax_window_page(struct fxfs_dax *dax, u64 window_offset)
{
	void *kaddr;

	if (dax_direct_access(dax->dax_dev, window_offset >> PAGE_SHIFT, 1, DAX_ACCESS, &kaddr,
			      NULL) < 1)
		return NULL;
	return kaddr;
}

/*
 * Maps a write of up to `length` bytes at `pos` writably, making room for it in open extents
 * unless its pages are writable already, and returns in the gate, which fxfs_dax_iomap_end leaves.
 * A zeroing write leaves holes be. A write past the file's end grows it, unless it is a fault's.
 */
/*
 * fxdax: with fxfs.dax_check_moves, WARNs unless `range`'s page `page`, where it moved, holds what
 * it held at `before_at` (`before`), but for the `len` bytes from `skip` a write writes (TASK-121).
 * In the gate, which keeps both still.
 */
static void fxfs_dax_check_moved(struct fxfs_dax *dax, struct inode *inode,
				 struct fxfs_dax_range *range, unsigned long page, const u8 *before,
				 u64 before_at, size_t skip, size_t len)
{
	u64 at = fxfs_dax_page_offset(range, page);
	int id = dax_read_lock();
	const u8 *now = fxfs_dax_window_page(dax, at);
	size_t i, first = PAGE_SIZE, differ = 0;

	for (i = 0; now && i < PAGE_SIZE; i++) {
		if ((i < skip || i >= skip + len) && now[i] != before[i]) {
			first = min(first, i);
			differ++;
		}
	}
	if (differ)
		pr_err("fxfs: ino %lu page %lu moved from %llx to %llx holds %zu other bytes, the first at %zu: %#x, not %#x\n",
		       inode->i_ino, (unsigned long)(fxfs_dax_chunk_start(range) >> PAGE_SHIFT) + page,
		       before_at, at, differ, first, now[first], before[first]);
	dax_read_unlock(id);
}

static int fxfs_dax_iomap_write(struct inode *inode, loff_t pos, loff_t length,
				unsigned int flags, struct iomap *iomap, struct iomap *srcmap)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_extent *extents = NULL;
	struct fxfs_dax_range *range;
	unsigned long first, pages;
	loff_t start, end, size, reserved;
	size_t capacity;
	bool fresh = false;
	/*
	 * A fault's page, which a store may change any part of, as it held it: copied here from its
	 * mapping, for Fxfs to write into the block it moves the page to, in the moving transaction,
	 * rather than read the old block through the device. fxdax: not copied back after the
	 * transaction, which a seal between made the new block's zeros durable (TASK-79.11).
	 */
	void *held = NULL;
	/* fxdax: the page as the window mapped it before it moved (fxfs.dax_check_moves). */
	void *before = NULL;
	u64 before_at = 0;
	bool fault = flags & IOMAP_FAULT;
	ssize_t count;
	long gate;
	u64 seals;
	int err = 0;

	/* Faults and zeroing don't grow the file. */
	size = flags & (IOMAP_FAULT | IOMAP_ZERO) ? 0 :
	       min_t(loff_t, pos + length, round_up(pos + 1, FXFS_DAX_SIZE));
	reserved = size ? fxfs_dax_append_end(inode, pos, size) : 0;
	end = max(reserved, min_t(loff_t, pos + length, round_up(pos + 1, FXFS_DAX_SIZE)));
	capacity = DIV_ROUND_UP(offset_in_page(pos) + end - pos, PAGE_SIZE);
	/* A chunk with no range yet: room for the write first. Zeroing leaves holes be. */
	if (!(flags & IOMAP_ZERO) && !xa_load(&file->ranges, pos >> FXFS_DAX_SHIFT)) {
		extents = fxfs_dax_alloc_extents(dax, capacity);
		if (!extents)
			return -ENOMEM;
		err = fxfs_dax_prepare_write(inode, pos,
					     min_t(loff_t, pos + length, round_up(pos + 1, FXFS_DAX_SIZE)) - pos,
					     size, reserved, fault, extents, capacity);
		if (err) {
			kvfree(extents);
			return err;
		}
		/* Its pages were holes, which mappings of the file may map zeros for. */
		fresh = true;
	}
	range = fxfs_dax_get_range(inode, pos >> FXFS_DAX_SHIFT, max(size, reserved), fault);
	/* Nor may reserving window space refuse a write whose own range fits. */
	if (IS_ERR(range) && reserved > size &&
	    (PTR_ERR(range) == -ENOMEM || PTR_ERR(range) == -ENOSPC))
		range = fxfs_dax_get_range(inode, pos >> FXFS_DAX_SHIFT, size, fault);
	if (IS_ERR(range)) {
		kvfree(extents);
		return PTR_ERR(range);
	}
	start = fxfs_dax_chunk_start(range);
	end = min_t(loff_t, pos + length, start + ((loff_t)range->pages << PAGE_SHIFT));
	/*
	 * A range covers the file's chunk as its size grows (fxfs_dax_fit). fxdax: unless a fault
	 * mapped it again as small as the file was, between the write's fit and its iomap: dropped,
	 * as a slice the write can't use is, and mapped again as large as it needs (-EXDEV). It was
	 * a WARN and EIO (TASK-79.28).
	 */
	if (end <= pos) {
		kvfree(extents);
		fxfs_dax_put_range(dax, range);
		return -EXDEV;
	}
	if (size)
		size = end;
	reserved = max(end, min(reserved, start + ((loff_t)range->pages << PAGE_SHIFT)));
	first = (pos - start) >> PAGE_SHIFT;
	pages = DIV_ROUND_UP(end - start, PAGE_SIZE) - first;

	if (!down_write_trylock(&file->sem)) {
		u64 began = ktime_get_ns();

		/* fxdax: a fault waits for it without its DAX entry (fxfs_dax_down_read). */
		if (fault) {
			kvfree(extents);
			fxfs_dax_put_range(dax, range);
			return -EDEADLK;
		}
		down_write(&file->sem);
		atomic64_inc(&dax->write_waits);
		atomic64_add(ktime_get_ns() - began, &dax->write_wait_ns);
	}
	for (;;) {
		gate = fxfs_dax_store_begin(dax, fault);
		if (gate < 0) {
			err = gate;
			goto out;
		}
		/*
		 * Writable pages take the write as they are. An append's size goes with the inode's
		 * next writeback, not a transaction of its own for every write (TASK-92.28).
		 */
		if (find_next_zero_bit(range->writable, first + pages, first) == first + pages) {
			if (size > i_size_read(inode)) {
				i_size_write(inode, size);
				__mark_inode_dirty(inode, I_DIRTY_SYNC | I_DIRTY_DATASYNC);
			}
			break;
		}
		if (flags & IOMAP_ZERO && find_next_bit(range->mapped, first + pages, first) ==
						  first + pages) {
			fxfs_device_store_end(dax->dev);
			up_write(&file->sem);
			fxfs_dax_put_range(dax, range);
			fxfs_dax_hole(iomap, end - pos);
			return 0;
		}
		fxfs_device_store_end(dax->dev);
		/* COW must keep the appended bytes too, beyond Fxfs's former end. */
		err = fxfs_dax_write_size_locked(inode);
		if (err)
			goto out;

		if (!extents) {
			extents = fxfs_dax_alloc_extents(dax, capacity);
			if (!extents) {
				err = -ENOMEM;
				goto out;
			}
		}
		fresh |= find_next_zero_bit(range->mapped, first + pages, first) != first + pages;
		if (READ_ONCE(fxfs_dax_check_moves) && pages == 1 && test_bit(first, range->mapped) &&
		    !before) {
			int id = dax_read_lock();
			void *page;

			before_at = fxfs_dax_page_offset(range, first);
			page = fxfs_dax_window_page(dax, before_at);
			before = page ? kmalloc(PAGE_SIZE, GFP_NOFS) : NULL;
			if (before)
				memcpy(before, page, PAGE_SIZE);
			dax_read_unlock(id);
		}
		if (fault && pages == 1 && test_bit(first, range->mapped) && !held) {
			int id;

			fxfs_dax_pause(FXFS_DAX_STEP_HELD);
			id = dax_read_lock();
			void *page = fxfs_dax_window_page(dax, fxfs_dax_page_offset(range, first));

			held = page && !fxfs_dax_fail_held ? kmalloc(PAGE_SIZE, GFP_NOFS) : NULL;
			if (held) {
				loff_t page_pos = start + ((loff_t)first << PAGE_SHIFT);
				loff_t in_file = clamp_t(loff_t, i_size_read(inode) - page_pos, 0,
							 PAGE_SIZE);

				/* What the page holds past the file's end isn't the file's. */
				memcpy(held, page, in_file);
				memset(held + in_file, 0, PAGE_SIZE - in_file);
			}
			dax_read_unlock(id);
		}
		/* fxdax: fault injection, as fxfs_dax_map_write waiting for compaction's seal. */
		if (fault && READ_ONCE(fxfs_dax_fault_seals)) {
			err = fxfs_nofs(fxfs_fsync(dax->fs));
			if (err)
				goto out;
		}
		/* As in fxfs_dax_map_chunk: unless a seal was in progress, or began since. */
		seals = fxfs_device_seals(dax->dev);
		/* A fault's pages hold what they held: a store may change any part of them. */
		count = fxfs_dax_reserve_write(inode, pos, end, reserved, size, fault, held,
					     extents, capacity);
		fxfs_dax_delay();
		if (fault && count >= 0)
			fxfs_dax_pause(FXFS_DAX_STEP_MOVED);
		if (count < 0) {
			err = count;
			goto out;
		}
		/* fxdax: fault injection, a seal between the move and the mapping of the new block. */
		if (fault && fxfs_dax_seal_moved()) {
			err = fxfs_nofs(fxfs_fsync(dax->fs));
			if (err)
				goto out;
		}
		if (size > i_size_read(inode))
			i_size_write(inode, size);
		gate = fxfs_dax_store_begin(dax, fault);
		if (gate == seals) {
			err = fxfs_dax_map_extents(dax, range, extents, count);
			if (!err && before)
				fxfs_dax_check_moved(dax, inode, range, first, before, before_at,
						     fault ? 0 : offset_in_page(pos),
						     fault ? 0 : min_t(loff_t, end - pos,
								       PAGE_SIZE - offset_in_page(pos)));
			if (err) {
				fxfs_device_store_end(dax->dev);
				/*
				 * fxdax: the blocks moved, but the range maps some where they were:
				 * dropped, and mapped again from where Fxfs has them (TASK-79.106).
				 */
				if (err != -EXDEV)
					err = -ESTALE;
				goto out;
			}
			break;
		}
		if (gate >= 0)
			fxfs_device_store_end(dax->dev);
		else
			fxfs_dax_pause(FXFS_DAX_STEP_SHUT);
		/*
		 * fxdax: a seal came between the move and the gate. A write holds the file's
		 * semaphore, and moves the blocks again, from where Fxfs has them now. A fault, its
		 * gate shut, gives up its page to wait for the seal, and with it the semaphore: the
		 * range maps the blocks where they moved, read-only, first (TASK-121).
		 */
		if (gate >= 0)
			continue;
		err = fxfs_dax_map_moved(dax, range, extents, count);
		if (!err)
			err = gate;
		goto out;
	}
	kfree(held);
	kfree(before);
	kvfree(extents);
	fxfs_dax_shadow_check(dax, range, first, pages, fault ? "a fault" : "a write");
	iomap->type = IOMAP_MAPPED;
	/* Holes a mapping of the file maps zeros for are gone. */
	iomap->flags |= IOMAP_F_PRIVATE | (fresh ? IOMAP_F_NEW : 0);
	iomap->addr = fxfs_dax_page_offset(range, first) + offset_in_page(pos);
	/* The pages in a row in the window: the next iomap maps those after. */
	iomap->length = min_t(loff_t, end, start + ((loff_t)(first + fxfs_dax_run(range, first,
								  first + pages)) << PAGE_SHIFT)) - pos;
	iomap->private = range;
	fxfs_dax_cow(inode, range, first, DIV_ROUND_UP(offset_in_page(pos) + iomap->length, PAGE_SIZE),
		     flags, iomap, srcmap);
	up_write(&file->sem);
	return 0;
out:
	up_write(&file->sem);
	kfree(held);
	kfree(before);
	kvfree(extents);
	fxfs_dax_put_range(dax, range);
	return err;
}

static int fxfs_dax_iomap_begin(struct inode *inode, loff_t pos, loff_t length,
				unsigned int flags, struct iomap *iomap, struct iomap *srcmap)
{
	int err;

	if (WARN_ON_ONCE(flags & IOMAP_REPORT))
		return -EIO;
	iomap->offset = pos;
	iomap->flags = 0;
	iomap->bdev = NULL;
	iomap->dax_dev = fxfs_dax_of(inode)->dax_dev;
	if (flags & (IOMAP_WRITE | IOMAP_ZERO)) {
		/* fxdax: a follower's filesystem is read-only: nothing writes. */
		if (WARN_ON_ONCE(fxfs_vfs_follows(inode->i_sb)))
			return -EROFS;
		return fxfs_dax_iomap_write(inode, pos, length, flags, iomap, srcmap);
	}
	err = fxfs_dax_iomap_read(inode, pos, length, flags, iomap, srcmap);
	/* fxdax: between the iomap and the read of what it maps, which its range keeps mapped. */
	if (!err)
		fxfs_dax_delay();
	return err;
}

static int fxfs_dax_iomap_end(struct inode *inode, loff_t pos, loff_t length, ssize_t written,
			      unsigned int flags, struct iomap *iomap)
{
	struct fxfs_dax *dax = fxfs_dax_of(inode);

	if (iomap->flags & IOMAP_F_PRIVATE)
		fxfs_device_store_end(dax->dev);
	if (iomap->private)
		fxfs_dax_put_range(dax, iomap->private);
	return 0;
}

static const struct iomap_ops fxfs_dax_iomap_ops = {
	.iomap_begin = fxfs_dax_iomap_begin,
	.iomap_end = fxfs_dax_iomap_end,
};

static ssize_t fxfs_dax_read(struct kiocb *iocb, struct iov_iter *to)
{
	return dax_iomap_rw(iocb, to, &fxfs_dax_iomap_ops);
}

/*
 * A read(2), holding the inode's lock shared. fxdax: a follower's, a chunk at a time under its
 * fill barrier, taken after the inode's lock, whose copies to the reader's memory don't fault
 * (fxfs_vfs_follower_read): no batch drops the extents it copies from, nor applies, until it is
 * done with them.
 */
static ssize_t fxfs_dax_read_iter(struct kiocb *iocb, struct iov_iter *to)
{
	struct inode *inode = file_inode(iocb->ki_filp);
	ssize_t ret;

	if (!iov_iter_count(to))
		return 0;
	if (iocb->ki_flags & IOCB_NOWAIT) {
		if (!inode_trylock_shared(inode))
			return -EAGAIN;
	} else {
		inode_lock_shared(inode);
	}
	if (fxfs_vfs_follows(inode->i_sb))
		ret = fxfs_vfs_follower_read(iocb, to, fxfs_dax_read);
	else
		ret = fxfs_dax_read(iocb, to);
	inode_unlock_shared(inode);
	file_accessed(iocb->ki_filp);
	return ret;
}

/*
 * Sets the file's size, after a write made it longer than what it wrote: as setattr would,
 * with the inode locked.
 */
static int fxfs_dax_set_size(struct inode *inode, loff_t size)
{
	struct fxfs_setattr set = { .valid = FXFS_SET_SIZE, .size = size };
	int err = fxfs_dax_resize_begin(inode, size);

	if (err)
		return err;
	truncate_setsize(inode, size);
	err = fxfs_nofs(fxfs_setattr(fxfs_vfs_fs(inode->i_sb), inode->i_ino, &set));
	fxfs_dax_resize_end(inode, size);
	return err;
}

/*
 * Writes through the window, copying with page faults disabled: in the gate, a fault on the
 * user's memory could wait for a seal that waits for the gate. The memory is faulted in first, and
 * again whenever a copy stops short.
 */
static ssize_t fxfs_dax_write_iter(struct kiocb *iocb, struct iov_iter *from)
{
	struct inode *inode = file_inode(iocb->ki_filp);
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	loff_t old_size, write_pos = 0;
	u64 seals = 0;
	ssize_t ret, done = 0;
	unsigned int stale = 0;
	int err;

	if (iocb->ki_flags & IOCB_NOWAIT)
		return -EAGAIN;
	inode_lock(inode);
	old_size = i_size_read(inode);
	ret = generic_write_checks(iocb, from);
	if (ret <= 0)
		goto out;
	ret = file_remove_privs(iocb->ki_filp);
	if (ret)
		goto out;
	ret = file_update_time(iocb->ki_filp);
	if (ret)
		goto out;
	write_pos = iocb->ki_pos;
	seals = fxfs_device_seals(fxfs_dax_of(inode)->dev);
	if (write_pos != old_size || file->append_next != write_pos || file->append_seals != seals)
		file->append_bytes = 0;
	/* The ranges the file's end moves into, mapped as large as they need. */
	if (iocb->ki_pos + iov_iter_count(from) > old_size) {
		filemap_invalidate_lock(inode->i_mapping);
		ret = fxfs_dax_fit(inode, iocb->ki_pos + iov_iter_count(from));
		filemap_invalidate_unlock(inode->i_mapping);
		if (ret)
			goto out;
	}
	while (iov_iter_count(from)) {
		if (fault_in_iov_iter_readable(from, iov_iter_count(from)) == iov_iter_count(from)) {
			ret = -EFAULT;
			break;
		}
		pagefault_disable();
		ret = dax_iomap_rw(iocb, from, &fxfs_dax_iomap_ops);
		pagefault_enable();
		if (ret > 0) {
			done += ret;
		} else if (ret == -EXDEV || (ret == -ESTALE && !stale++)) {
			/*
			 * A slice the write can't use, or fxdax: a range mapping some of its blocks
			 * where they were (TASK-79.106), once: dropped, the chunk is mapped again.
			 */
			ret = fxfs_dax_drop_slice(inode, iocb->ki_pos, false);
			if (ret)
				break;
		} else if (ret != -EFAULT) {
			if (ret == -ESTALE)
				ret = -EIO;
			break;
		}
		/* What faulted in may be gone again: the loop faults it in once more. */
	}
	/* A write that stopped short leaves the file no longer than it wrote. */
	if (i_size_read(inode) > max(old_size, iocb->ki_pos)) {
		err = fxfs_dax_set_size(inode, max(old_size, iocb->ki_pos));
		if (err && !done)
			ret = err;
	}
	if (done) {
		ret = done;
		file->append_bytes = write_pos == old_size &&
			seals == fxfs_device_seals(fxfs_dax_of(inode)->dev) ?
			min_t(u64, 2 * PAGE_SIZE, file->append_bytes + done) : 0;
		file->append_next = iocb->ki_pos;
		file->append_seals = seals;
	}
out:
	inode_unlock(inode);
	if (ret > 0)
		ret = generic_write_sync(iocb, ret);
	return ret;
}

/* A fault of `order`, PAGE's or PMD's, writing if `write`. */
static vm_fault_t fxfs_dax_do_fault(struct vm_fault *vmf, unsigned int order, bool write)
{
	struct inode *inode = file_inode(vmf->vma->vm_file);
	struct super_block *sb = inode->i_sb;
	unsigned int stale = 0;
	vm_fault_t ret;
	unsigned long pfn;
	int error;

	if (write) {
		sb_start_pagefault(sb);
		file_update_time(vmf->vma->vm_file);
	}
	for (;;) {
		long seen = atomic_long_read(&fxfs_dax_of(inode)->released);

		/*
		 * fxdax: each try after the first yields first: retries that find the file's semaphore,
		 * or the gate, busy as soon as they look again kept the task holding it off both of a
		 * 2-vCPU guest's CPUs, without preemption, for minutes (TASK-113).
		 */
		cond_resched();
		error = 0;
		/* fxdax: a follower's extents read and mapped under its fill barrier, as of one state. */
		fxfs_vfs_fill_begin(sb);
		filemap_invalidate_lock_shared(inode->i_mapping);
		ret = dax_iomap_fault(vmf, order, &pfn, &error, &fxfs_dax_iomap_ops);
		filemap_invalidate_unlock_shared(inode->i_mapping);
		fxfs_vfs_fill_end(sb);
		if (!(ret & VM_FAULT_ERROR) || (error != -EAGAIN && error != -EXDEV &&
						 error != -EBUSY && error != -ESTALE && error != -EDEADLK))
			break;
		/* fxdax: a fault killed meanwhile tries no more (TASK-79.49). */
		if (fatal_signal_pending(current)) {
			ret = VM_FAULT_SIGBUS;
			break;
		}
		/* fxdax: a range mapping blocks where they were, dropped once (TASK-79.106). */
		if (error == -ESTALE) {
			if (stale++ || fxfs_dax_drop_slice(inode, (loff_t)vmf->pgoff << PAGE_SHIFT,
							   true))
				break;
			continue;
		}
		if (error == -EBUSY) {
			/* A seal was in progress: waited for without the page's DAX entry. */
			fxfs_device_wait_unsealed(fxfs_dax_of(inode)->dev);
			continue;
		}
		if (error == -EDEADLK) {
			/*
			 * fxdax: the file's semaphore was held: waited for without the page's DAX
			 * entry, which a write(2) its holder waits for may wait for (TASK-79.12).
			 */
			struct fxfs_dax_file *file = fxfs_dax_file(inode);

			down_read(&file->sem);
			up_read(&file->sem);
			continue;
		}
		if (error == -EXDEV) {
			/* A slice can't take the store: it maps the chunk no more. */
			if (fxfs_dax_drop_slice(inode, (loff_t)vmf->pgoff << PAGE_SHIFT, true))
				break;
			continue;
		}
		/* No range was free, nor could the fault reclaim one, holding the lock it holds. */
		fxfs_dax_make_room(fxfs_dax_of(inode), seen);
	}
	/* fxdax: a SIGBUS says nothing of why: the error here does (TASK-121). */
	if (ret & VM_FAULT_SIGBUS)
		pr_err_ratelimited("fxfs: a %s fault of order %u at page %lu of ino %lu failed: %d\n",
				   write ? "write" : "read", order, vmf->pgoff, inode->i_ino, error);
	if (write)
		sb_end_pagefault(sb);
	return ret;
}

/* A write to a private mapping writes its own copy of the page, which the fault reads: as ext4's. */
static vm_fault_t fxfs_dax_fault(struct vm_fault *vmf)
{
	return fxfs_dax_do_fault(vmf, 0, (vmf->flags & FAULT_FLAG_WRITE) &&
						 (vmf->vma->vm_flags & VM_SHARED));
}

/*
 * fxdax: a read fault of a PMD-aligned 2 MiB of a mapping maps them with one PMD, read-only, where
 * the window holds them in a row, as it does a chunk of a large file mapped whole (the iomap says
 * so); otherwise the kernel faults them a page at a time (VM_FAULT_FALLBACK, TASK-26.18.8). A store
 * falls back to pages, so that no PMD is ever writable: the sealer write-protects user mappings
 * page by page (wp_shared_mapping_range), which would leave a writable PMD writable.
 */
static vm_fault_t fxfs_dax_huge_fault(struct vm_fault *vmf, unsigned int order)
{
	if (order != PMD_ORDER || (vmf->flags & FAULT_FLAG_WRITE))
		return VM_FAULT_FALLBACK;
	return fxfs_dax_do_fault(vmf, order, false);
}

static vm_fault_t fxfs_dax_mkwrite(struct vm_fault *vmf)
{
	return fxfs_dax_do_fault(vmf, 0, true);
}

static const struct vm_operations_struct fxfs_dax_vm_ops = {
	.fault = fxfs_dax_fault,
	.huge_fault = fxfs_dax_huge_fault,
	.page_mkwrite = fxfs_dax_mkwrite,
	.pfn_mkwrite = fxfs_dax_mkwrite,
};

static int fxfs_dax_mmap(struct file *file, struct vm_area_struct *vma)
{
	file_accessed(file);
	vma->vm_ops = &fxfs_dax_vm_ops;
	/* fxdax: read faults may map 2 MiB at once (fxfs_dax_huge_fault), as ext4's DAX does. */
	vm_flags_set(vma, VM_MIXEDMAP | VM_HUGEPAGE);
	return 0;
}

static int fxfs_dax_writepages(struct address_space *mapping, struct writeback_control *wbc)
{
	return dax_writeback_mapping_range(mapping, fxfs_dax_of(mapping->host)->dax_dev, wbc);
}

static const struct address_space_operations fxfs_dax_aops = {
	.writepages = fxfs_dax_writepages,
	.dirty_folio = noop_dirty_folio,
	/* O_DIRECT opens: reads and writes through the window are direct already, as ext4's. */
	.direct_IO = noop_direct_IO,
};

static const struct file_operations fxfs_dax_file_operations = {
	.open = fxfs_vfs_file_open,
	.release = fxfs_vfs_file_release,
	.llseek = fxfs_vfs_llseek,
	.read_iter = fxfs_dax_read_iter,
	.write_iter = fxfs_dax_write_iter,
	.mmap = fxfs_dax_mmap,
	/* fxdax: mappings 2 MiB-aligned, as the window's chunks are, for PMD faults. */
	.get_unmapped_area = thp_get_unmapped_area,
	.splice_read = copy_splice_read,
	.splice_write = iter_file_splice_write,
	.fsync = fxfs_vfs_fsync,
	.fallocate = fxfs_vfs_fallocate,
	.remap_file_range = fxfs_vfs_remap_file_range,
};

/*
 * fxdax: with fxfs.dax_check_seals, WARNs of a range with pages writable after a seal's sealer,
 * no store in progress: its file wasn't on the writable files, whose pages the sealer forgets
 * and the user's mappings of which it makes read-only (TASK-121).
 */
struct fxfs_dax_writable_ptes {
	unsigned long count, addr;
	pgoff_t pgoff;
	/* fxdax: with the shadow, the window's first pfn, and the writable PTEs it disowns. */
	struct fxfs_dax *dax;
	unsigned long base, disowned;
	u64 file;
};

static int fxfs_dax_writable_pte(pte_t *pte, unsigned long addr, unsigned long next,
				 struct mm_walk *walk)
{
	struct fxfs_dax_writable_ptes *found = walk->private;
	pte_t entry = ptep_get(pte);
	unsigned long page;
	u64 seals, then;
	void *shadow;

	if (!pte_present(entry) || !pte_write(entry))
		return 0;
	if (!found->count++) {
		found->addr = addr;
		found->pgoff = linear_page_index(walk->vma, addr);
	}
	if (!found->dax)
		return 0;
	/* A writable PTE of a window page the guest mapped writably this seal, or none. */
	page = pte_pfn(entry) - found->base;
	shadow = xa_load(&found->dax->shadow, page);
	seals = fxfs_device_seals(found->dax->dev);
	then = shadow ? xa_to_value(shadow) >> 1 : 0;
	if (shadow && (xa_to_value(shadow) & 1) && (then == seals || then == seals - 1))
		return 0;
	if (found->disowned++ < 8)
		pr_err("fxfs: ino %llu page %lu mapped writably at %lx, window page %lx, which the guest %s (seals %llu, then %llu)\n",
		       found->file, linear_page_index(walk->vma, addr), addr, page,
		       !shadow ? "never mapped" : !(xa_to_value(shadow) & 1) ? "mapped read-only" :
		       "mapped writably before a seal", seals, then);
	return 0;
}

static const struct mm_walk_ops fxfs_dax_writable_ops = {
	.pte_entry = fxfs_dax_writable_pte,
	.walk_lock = PGWALK_RDLOCK,
};

/*
 * fxdax: with fxfs.dax_shadow, as a seal begins, before its sealer makes them read-only: logs the
 * user's writable PTEs of window pages the guest didn't map writably this seal, whose stores the
 * window refuses (TASK-121).
 */
static void fxfs_dax_check_ptes(struct fxfs_dax *dax)
{
	struct fxfs_dax_writable_ptes found;
	struct fxfs_dax_file *file;
	unsigned long ino, base;
	void *kaddr;
	int id;

	if (!READ_ONCE(fxfs_dax_shadowed))
		return;
	id = dax_read_lock();
	if (dax_direct_access(dax->dax_dev, 0, 1, DAX_ACCESS, &kaddr, &base) < 1)
		base = 0;
	dax_read_unlock(id);
	if (!base)
		return;
	mutex_lock(&dax->files_lock);
	xa_for_each(&dax->files, ino, file) {
		struct address_space *mapping;

		if (!file->inode)
			continue;
		mapping = file->inode->i_mapping;
		found = (struct fxfs_dax_writable_ptes) { .dax = dax, .base = base, .file = file->ino };
		i_mmap_lock_read(mapping);
		walk_page_mapping(mapping, 0, ULONG_MAX, &fxfs_dax_writable_ops, &found);
		i_mmap_unlock_read(mapping);
	}
	mutex_unlock(&dax->files_lock);
}

static void fxfs_dax_check_sealed(struct fxfs_dax *dax)
{
	struct fxfs_dax_writable_ptes found;
	struct fxfs_dax_range *range;
	struct fxfs_dax_file *file;
	unsigned long ino, chunk;

	mutex_lock(&dax->files_lock);
	xa_for_each(&dax->files, ino, file) {
		/* The user's mappings of its pages, none writable after the sealer. */
		if (file->inode) {
			struct address_space *mapping = file->inode->i_mapping;

			found = (struct fxfs_dax_writable_ptes) {};
			i_mmap_lock_read(mapping);
			walk_page_mapping(mapping, 0, ULONG_MAX, &fxfs_dax_writable_ops, &found);
			i_mmap_unlock_read(mapping);
			WARN(found.count,
			     "fxfs: ino %llu: %lu user mappings writable after a seal, the first at %lx, page %lu (%s the writable files)\n",
			     file->ino, found.count, found.addr, found.pgoff,
			     list_empty(&file->writable) ? "not on" : "on");
		}
		rcu_read_lock();
		xa_for_each(&file->ranges, chunk, range) {
			unsigned long page = find_first_bit(range->writable, range->pages);

			WARN(page < range->pages,
			     "fxfs: ino %llu chunk %lu page %lu writable after a seal (%s, slice %d)\n",
			     file->ino, chunk, page, file->inode ? "inode" : "no inode",
			     !!range->region);
		}
		rcu_read_unlock();
	}
	mutex_unlock(&dax->files_lock);
}

/*
 * Before a seal: makes user mappings of writable pages read-only, and forgets which pages are
 * writable, and which regions, whose extents grow no more: their space in the window past what
 * they map is free again. With no store in progress.
 */
static void fxfs_dax_seal(void *arg)
{
	struct fxfs_dax *dax = arg;
	struct fxfs_dax_region *region, *next_region;
	struct fxfs_dax_file *file, *next;
	struct fxfs_dax_range *range;
	unsigned long chunk;

	fxfs_dax_pause(FXFS_DAX_STEP_SEALING);
	fxfs_dax_check_ptes(dax);
	spin_lock(&dax->regions_lock);
	list_for_each_entry_safe(region, next_region, &dax->writable_regions, writable_list) {
		region->writable = false;
		region->evacuate = true;
		list_del_init(&region->writable_list);
		fxfs_dax_give_window(dax, region->window_offset + ((u64)region->mapped << PAGE_SHIFT),
				     region->pages - region->mapped);
		region->pages = region->mapped;
	}
	spin_unlock(&dax->regions_lock);
	/* fxdax: between the regions sealed and the files' pages (TASK-119.3). */
	fxfs_dax_delay();

	mutex_lock(&dax->writable_lock);
	list_for_each_entry_safe(file, next, &dax->writable, writable) {
		/* A file with no inode has nothing mapping its pages but the window. */
		mutex_lock(&dax->files_lock);
		if (file->inode)
			wp_shared_mapping_range(file->inode->i_mapping, 0, ULONG_MAX);
		mutex_unlock(&dax->files_lock);
		/* Ranges freed meanwhile are freed after a grace period (fxfs_dax_free_range). */
		rcu_read_lock();
		xa_for_each(&file->ranges, chunk, range)
			bitmap_zero(range->writable, range->pages);
		rcu_read_unlock();
		list_del_init(&file->writable);
		/* fxdax: its pages in the staging extent's region move once the seal ends. */
		if (READ_ONCE(dax->restaging) && READ_ONCE(fxfs_dax_evacuates) &&
		    list_empty(&file->evacuating) &&
		    refcount_inc_not_zero(&file->refs))
			list_add_tail(&file->evacuating, &dax->evacuating);
	}
	mutex_unlock(&dax->writable_lock);
	if (READ_ONCE(fxfs_dax_check_seals))
		fxfs_dax_check_sealed(dax);
}

/*
 * fxdax: once a seal ends, has the staging extent the seal sealed replaced in the background, if
 * writes took blocks of it, and the new one's region mapped (fxfs_dax_restage_work).
 */
static void fxfs_dax_sealed(void *arg)
{
	struct fxfs_dax *dax = arg;

	/* fxdax: between the seal's end and what follows it (TASK-119.3). */
	fxfs_dax_delay();
	if (READ_ONCE(dax->restaging) && queue_work(dax->grow_wq, &dax->restage))
		atomic_inc(&dax->growing);
	if (READ_ONCE(dax->restaging))
		queue_work(dax->grow_wq, &dax->evacuate);
}

/*
 * fxdax: replaces the staging extent sealed, if writes took blocks of it, and maps the new one's
 * region writably, as the first write to it would: so that the next writes, whose blocks move
 * there, wait for no MAP, as a database's log fsynced at each transaction would at each. It keeps
 * the region until the next replaces it. The new extent is open only while no seal comes between
 * it and its mapping: if one does, it isn't mapped, and the allocator takes another.
 */
static void fxfs_dax_restage_work(struct work_struct *work)
{
	struct fxfs_dax *dax = container_of(work, struct fxfs_dax, restage);
	struct fxfs_dax_region *region, *old;
	struct fxfs_dax_extent e;
	u64 seals = fxfs_device_seals(dax->dev);

	if (!fxfs_nofs(fxfs_dax_restage(dax->fs, &e)))
		goto out;
	region = fxfs_dax_get_region(dax, &e, false);
	if (IS_ERR(region))
		goto out;
	if (fxfs_device_store_begin(dax->dev) == seals) {
		fxfs_dax_region_open(dax, region);
		if (READ_ONCE(region->writable))
			fxfs_dax_map_to(dax, region, 1, FXFS_DAX_RESTAGE);
	}
	fxfs_device_store_end(dax->dev);
	spin_lock(&dax->regions_lock);
	old = dax->spare;
	dax->spare = region;
	spin_unlock(&dax->regions_lock);
	if (old)
		fxfs_dax_put_region(dax, old);
out:
	atomic_dec(&dax->growing);
}

/*
 * fxdax: the longest run of `pool`'s free space, in bytes: of a window free in pieces, the most a
 * range or a region can take (TASK-91.10). Read as it changes.
 */
static size_t fxfs_dax_largest_free(struct gen_pool *pool)
{
	unsigned long nbits, start, end, largest = 0;
	struct gen_pool_chunk *chunk;

	rcu_read_lock();
	list_for_each_entry_rcu(chunk, &pool->chunks, next_chunk) {
		nbits = (chunk->end_addr - chunk->start_addr + 1) >> pool->min_alloc_order;
		for (start = find_first_zero_bit(chunk->bits, nbits); start < nbits;
		     start = find_next_zero_bit(chunk->bits, nbits, end)) {
			end = find_next_bit(chunk->bits, nbits, start);
			largest = max(largest, end - start);
		}
	}
	rcu_read_unlock();
	return (size_t)largest << pool->min_alloc_order;
}

/*
 * fxdax: a run of a range's pages in a sealed staging extent's region, moving to the range's own
 * space (fxfs_dax_evacuate_work): `moved` its first page's place there, the rest following it.
 */
struct fxfs_dax_evacuee {
	struct fxfs_dax_file *file;
	struct fxfs_dax_range *range;
	unsigned long page, pages;
	u64 moved;
	/* Whether it holds the range for the evacuation, its first run; and whether it was mapped. */
	bool holds, mapped;
};

/* fxdax: the runs of an evacuation, and their maps, as many of each. */
struct fxfs_dax_evacuation {
	struct fxfs_dax_evacuee *runs;
	struct fxfs_device_map *maps;
	size_t count, capacity;
};

/* fxdax: one more run in `ev`, room made for it: false without memory. */
static bool fxfs_dax_evacuee_add(struct fxfs_dax_evacuation *ev, struct fxfs_dax_evacuee run,
				 struct fxfs_device_map map)
{
	if (ev->count == ev->capacity) {
		size_t capacity = max_t(size_t, 16, 2 * ev->capacity);
		struct fxfs_dax_evacuee *runs;
		struct fxfs_device_map *maps;

		runs = krealloc_array(ev->runs, capacity, sizeof(*runs), GFP_NOFS);
		if (!runs)
			return false;
		ev->runs = runs;
		maps = krealloc_array(ev->maps, capacity, sizeof(*maps), GFP_NOFS);
		if (!maps)
			return false;
		ev->maps = maps;
		ev->capacity = capacity;
	}
	ev->runs[ev->count] = run;
	ev->maps[ev->count++] = map;
	return true;
}

/*
 * fxdax: gathers `file`'s runs of pages in sealed staging extents' regions into `ev`, holding each
 * range that has some for the evacuation (its users), as an iomap does, so that it isn't freed nor
 * its space taken by another before the evacuation ends. With the file's semaphore held for
 * writing.
 */
static void fxfs_dax_evacuee_gather(struct fxfs_dax_file *file, struct fxfs_dax_evacuation *ev)
{
	struct fxfs_dax_region *region;
	struct fxfs_dax_range *range;
	unsigned long chunk, page, end;
	bool held;
	u64 moved;

	lockdep_assert_held_write(&file->sem);
	xa_for_each(&file->ranges, chunk, range) {
		if (range->region || !range->moved)
			continue;
		held = false;
		for (page = 0; page < range->pages; page = end) {
			end = page + 1;
			moved = range->moved[page];
			if (!moved)
				continue;
			region = range->regions[(moved >> 32) - 1];
			if (!READ_ONCE(region->evacuate))
				continue;
			while (end < range->pages && range->moved[end] == moved + (end - page))
				end++;
			if (!fxfs_dax_evacuee_add(ev, (struct fxfs_dax_evacuee) {
					.file = file, .range = range, .page = page,
					.pages = end - page, .moved = moved, .holds = !held,
				}, (struct fxfs_device_map) {
					.branch = region->branch, .sequence = region->sequence,
					.offset = (moved & U32_MAX) << PAGE_SHIFT,
					.len = (u64)(end - page) << PAGE_SHIFT,
					.window_offset = range->window_offset + ((u64)page << PAGE_SHIFT),
				}))
				return;
			if (!held) {
				atomic_inc(&range->users);
				atomic_inc(&file->held);
			}
			held = true;
		}
	}
}

/*
 * fxdax: maps `file`'s runs in `ev` in their ranges' own space, read-only, in one request
 * (MAP_MANY), after a delay if fxfs.dax_delay_us asks for them: a file with no runs is no
 * slower to evacuate. With the file's semaphore held for writing.
 */
static void fxfs_dax_evacuee_map(struct fxfs_dax *dax, struct fxfs_dax_file *file,
				 struct fxfs_dax_evacuation *ev)
{
	u64 began;
	size_t r;
	int err;

	lockdep_assert_held_write(&file->sem);
	if (ev->count) {
		fxfs_dax_delay();
		began = ktime_get_ns();
		err = fxfs_nofs(fxfs_dax_device_map_many(dax, ev->maps, ev->count));
		atomic64_add(ktime_get_ns() - began, &dax->evacuate_map_ns);
		for (r = 0; r < ev->count; r++) {
			/* If they failed, mapped one by one, to place those that did. */
			if (err)
				ev->runs[r].mapped = !fxfs_nofs(fxfs_dax_device_map(
					dax, ev->maps[r].branch, ev->maps[r].sequence,
					ev->maps[r].offset, ev->maps[r].len, ev->maps[r].window_offset,
					false, false));
			else
				ev->runs[r].mapped = true;
			atomic64_inc(&dax->maps[FXFS_DAX_EVACUATE]);
			atomic64_add(ev->maps[r].len, &dax->map_bytes[FXFS_DAX_EVACUATE]);
		}
	}
}

/*
 * fxdax: has `file`'s runs in `ev` that were mapped in their ranges' own space be there, unless a
 * write moved them meanwhile, or another iomap uses their range, which may use a page where it
 * was; and lets go of the ranges, and of the regions they emptied (fxfs_dax_prune). Pages nothing
 * but the window maps aren't relocated for the kernel: there is nothing of them to replace. With
 * the file's semaphore held for writing.
 */
static void fxfs_dax_evacuee_place(struct fxfs_dax *dax, struct fxfs_dax_file *file,
				   struct fxfs_dax_evacuation *ev)
{
	struct fxfs_dax_evacuee *run;
	pgoff_t index;
	unsigned long i;
	bool moved;
	size_t r;

	lockdep_assert_held_write(&file->sem);
	mutex_lock(&dax->files_lock);
	for (r = 0; r < ev->count; r++) {
		run = &ev->runs[r];
		if (run->file != file || !run->mapped || atomic_read(&run->range->users) != 1)
			continue;
		moved = true;
		for (i = 0; i < run->pages; i++)
			moved &= run->range->moved[run->page + i] == run->moved + i;
		if (!moved)
			continue;
		fxfs_dax_relocate(run->range, run->page, run->pages, NULL, 0);
		index = (fxfs_dax_chunk_start(run->range) >> PAGE_SHIFT) + run->page;
		if (!file->inode || !xa_find(&file->inode->i_mapping->i_pages, &index,
					     index + run->pages - 1, XA_PRESENT))
			bitmap_clear(run->range->relocated, run->page, run->pages);
	}
	mutex_unlock(&dax->files_lock);
	for (r = 0; r < ev->count; r++) {
		run = &ev->runs[r];
		if (run->file != file || !run->holds)
			continue;
		fxfs_dax_prune(dax, run->range);
		atomic_dec(&file->held);
		fxfs_dax_put_range(dax, run->range);
	}
}

/*
 * fxdax: whether an iomap uses one of `file`'s ranges an evacuation would gather runs of. With
 * the file's semaphore held.
 */
static bool fxfs_dax_evacuees_used(struct fxfs_dax_file *file)
{
	struct fxfs_dax_range *range;
	unsigned long chunk;

	lockdep_assert_held(&file->sem);
	xa_for_each(&file->ranges, chunk, range) {
		if (!range->region && range->moved && atomic_read(&range->users))
			return true;
	}
	return false;
}

/*
 * fxdax: takes `file`'s semaphore for writing once no iomap uses the ranges an evacuation would
 * gather runs of: an iomap holds its range past the semaphore, through its copy, and one begun
 * just before the evacuation took the semaphore had the placing skip its runs, the regions they
 * emptied staying (regions_evacuated under fxfs.dax_delay_us, GCE r99). With the semaphore held,
 * no iomap takes the ranges until the evacuation ends. It waits without the semaphore, as an
 * iomap's copy may fault on the file, whose fault takes it.
 */
static void fxfs_dax_lock_evacuable(struct fxfs_dax *dax, struct fxfs_dax_file *file)
{
	long seen;

	for (;;) {
		down_write(&file->sem);
		seen = atomic_long_read(&dax->released);
		if (!fxfs_dax_evacuees_used(file))
			return;
		up_write(&file->sem);
		wait_event(dax->wait, atomic_long_read(&dax->released) != seen);
	}
}

/*
 * fxdax: evacuates `file`: gathers its runs, maps them in their ranges' own space, and has them be
 * there, holding its semaphore for writing throughout. So no write maps one of those pages anew in
 * its range's own space between the gathering and the MAP, which the evacuation's MAP, landing
 * after, would take the place of, the host then refusing the write's stores (TASK-101); nor does a
 * write's iomap come between the MAP and the placing, holding the range, which the placing would
 * skip, the regions the runs emptied staying (TASK-91.10). It holds no other file's, and the MAP
 * waits for nothing, so that a write waiting for the semaphore, in the gate, waits for this
 * evacuation alone.
 */
static void fxfs_dax_evacuate_file(struct fxfs_dax *dax, struct fxfs_dax_file *file)
{
	struct fxfs_dax_evacuation ev = {};
	u64 locked;

	fxfs_dax_lock_evacuable(dax, file);
	locked = ktime_get_ns();
	fxfs_dax_evacuee_gather(file, &ev);
	fxfs_dax_evacuee_map(dax, file, &ev);
	/* fxdax: between the MAP and the placing (TASK-119.3). */
	if (ev.count)
		fxfs_dax_delay();
	fxfs_dax_evacuee_place(dax, file, &ev);
	up_write(&file->sem);
	if (ev.count) {
		atomic64_inc(&dax->evacuations);
		atomic64_add(ktime_get_ns() - locked, &dax->evacuate_hold_ns);
	}
	/* Those waiting to drop its ranges (fxfs_dax_lock_unheld). */
	wake_up_all(&dax->wait);
	kfree(ev.runs);
	kfree(ev.maps);
}

/*
 * fxdax: takes `file`'s semaphore for writing once no evacuation holds its ranges: for the paths
 * that drop them whatever uses them, as an evicted unlinked file's and a truncate's, which no
 * iomap can use meanwhile, but an evacuation may (fxfs_dax_evacuee_gather).
 */
static void fxfs_dax_lock_unheld(struct fxfs_dax *dax, struct fxfs_dax_file *file)
{
	for (;;) {
		down_write(&file->sem);
		if (!atomic_read(&file->held))
			return;
		up_write(&file->sem);
		wait_event(dax->wait, !atomic_read(&file->held));
	}
}

/*
 * fxdax: once a seal ends, maps the pages ranges moved to the staging extents it sealed in the
 * ranges' own space, each file's in one request (fxfs_dax_evacuate_file), and has them be there,
 * so that the regions they emptied go: the window kept every staging extent's region a page
 * written once was in, a new one each seal, until its regions' space filled and every write
 * mapped its page alone
 * (TASK-91.10). The files whose pages a seal sealed meanwhile are evacuated again.
 */
static void fxfs_dax_evacuate_work(struct work_struct *work)
{
	struct fxfs_dax *dax = container_of(work, struct fxfs_dax, evacuate);
	struct fxfs_dax_file *file, *next;
	u64 seals = fxfs_device_seals(dax->dev);
	bool again = false;
	LIST_HEAD(files);

	/* Off the filesystem's list, onto this one: a seal meanwhile doesn't add them again. */
	mutex_lock(&dax->writable_lock);
	list_splice_init(&dax->evacuating, &files);
	mutex_unlock(&dax->writable_lock);
	list_for_each_entry(file, &files, evacuating)
		fxfs_dax_evacuate_file(dax, file);
	list_for_each_entry_safe(file, next, &files, evacuating) {
		mutex_lock(&dax->writable_lock);
		list_del_init(&file->evacuating);
		/* A seal since may have sealed more of its pages in a staging extent's region. */
		if (fxfs_device_seals(dax->dev) != seals && READ_ONCE(dax->restaging)) {
			list_add_tail(&file->evacuating, &dax->evacuating);
			again = true;
			file = NULL;
		}
		mutex_unlock(&dax->writable_lock);
		if (file)
			fxfs_dax_put_file(dax, file);
	}
	if (again)
		queue_work(dax->grow_wq, &dax->evacuate);
}

/* fxdax: lets go of the files left to evacuate, as the filesystem quiesces or goes. */
static void fxfs_dax_forget_evacuating(struct fxfs_dax *dax)
{
	struct fxfs_dax_file *file;

	for (;;) {
		mutex_lock(&dax->writable_lock);
		file = list_first_entry_or_null(&dax->evacuating, struct fxfs_dax_file, evacuating);
		if (file)
			list_del_init(&file->evacuating);
		mutex_unlock(&dax->writable_lock);
		if (!file)
			return;
		fxfs_dax_put_file(dax, file);
	}
}

void fxfs_dax_show_stats(struct fxfs_dax *dax, struct seq_file *m)
{
	bool apart = dax->region_pool != dax->pool;
	int why;

	seq_printf(m, "window_free_bytes %zu\n", gen_pool_avail(dax->pool) +
		   (apart ? gen_pool_avail(dax->region_pool) : 0));
	seq_printf(m, "window_largest_free_bytes %zu\n", fxfs_dax_largest_free(dax->pool));
	if (apart)
		seq_printf(m, "window_regions_free_bytes %zu\nwindow_regions_largest_free_bytes %zu\n",
			   gen_pool_avail(dax->region_pool),
			   fxfs_dax_largest_free(dax->region_pool));
	seq_printf(m, "window_full %ld\nwindow_regions_full %ld\nwindow_reclaims %ld\n",
		   atomic_long_read(&dax->window_full), atomic_long_read(&dax->regions_full),
		   atomic_long_read(&dax->reclaims));
	seq_printf(m, "window_regions %ld\nwindow_regions_pruned %ld\n",
		   atomic_long_read(&dax->region_count), atomic_long_read(&dax->regions_pruned));
	seq_printf(m, "dax_map_writes %lld\n", atomic64_read(&dax->map_writes));
	seq_printf(m, "dax_extent_alloc_bytes %lld\n", atomic64_read(&dax->extent_alloc_bytes));
	seq_printf(m, "dax_write_waits %lld\ndax_write_wait_ns %lld\n",
		   atomic64_read(&dax->write_waits), atomic64_read(&dax->write_wait_ns));
	seq_printf(m, "dax_moved_unsealed %lld\n", atomic64_read(&dax->moved_unsealed));
	seq_printf(m, "dax_evacuations %lld\ndax_evacuate_hold_ns %lld\ndax_evacuate_map_ns %lld\n",
		   atomic64_read(&dax->evacuations), atomic64_read(&dax->evacuate_hold_ns),
		   atomic64_read(&dax->evacuate_map_ns));
	for (why = 0; why < FXFS_DAX_WHYS; why++)
		seq_printf(m, "window_maps_%s %lld\nwindow_map_bytes_%s %lld\n", fxfs_dax_whys[why],
			   atomic64_read(&dax->maps[why]), fxfs_dax_whys[why],
			   atomic64_read(&dax->map_bytes[why]));
}

/* Destroys the window's pools: the ranges', and the regions', if apart. */
static void fxfs_dax_destroy_pools(struct fxfs_dax *dax)
{
	if (dax->region_pool && dax->region_pool != dax->pool)
		gen_pool_destroy(dax->region_pool);
	if (dax->pool)
		gen_pool_destroy(dax->pool);
}

struct fxfs_dax *fxfs_dax_create(struct super_block *sb, struct fxfs *fs)
{
	struct fxfs_device *dev = fxfs_dax_device(fs);
	struct dax_device *dax_dev;
	struct fxfs_dax *dax;
	u64 window_len;

	dax_dev = fxfs_device_dax(dev, &window_len);
	if (!dax_dev || window_len < FXFS_DAX_SIZE)
		return NULL;
	dax = kzalloc(sizeof(*dax), GFP_KERNEL);
	if (!dax)
		return ERR_PTR(-ENOMEM);
	dax->pool = gen_pool_create(PAGE_SHIFT, -1);
	/*
	 * fxdax: half a window of four chunks or more, in chunks, for regions (TASK-91.10): a quarter
	 * filled as pgbench -i -s 50 loaded its tables, whose writes then mapped their pieces in
	 * ranges' space, the load half as slow again.
	 */
	dax->regions_at = window_len < 4 * FXFS_DAX_SIZE ? window_len :
			  window_len - rounddown(window_len / 2, FXFS_DAX_SIZE);
	dax->region_pool = dax->regions_at < window_len ? gen_pool_create(PAGE_SHIFT, -1) : dax->pool;
	if (!dax->pool || !dax->region_pool ||
	    gen_pool_add(dax->pool, FXFS_DAX_POOL_BASE, dax->regions_at, -1) ||
	    (dax->region_pool != dax->pool &&
	     gen_pool_add(dax->region_pool, FXFS_DAX_POOL_BASE + dax->regions_at,
			  window_len - dax->regions_at, -1))) {
		fxfs_dax_destroy_pools(dax);
		kfree(dax);
		return ERR_PTR(-ENOMEM);
	}
	dax->sb = sb;
	dax->fs = fs;
	dax->dev = dev;
	dax->dax_dev = dax_dev;
	xa_init(&dax->files);
	xa_init(&dax->shadow);
	mutex_init(&dax->files_lock);
	spin_lock_init(&dax->lock);
	INIT_LIST_HEAD(&dax->used);
	init_waitqueue_head(&dax->wait);
	atomic_long_set(&dax->released, 0);
	mutex_init(&dax->writable_lock);
	INIT_LIST_HEAD(&dax->writable);
	hash_init(dax->regions);
	spin_lock_init(&dax->regions_lock);
	INIT_LIST_HEAD(&dax->writable_regions);
	atomic_set(&dax->growing, 0);
	dax->grow_wq = alloc_workqueue("fxfs_dax_grow", WQ_UNBOUND | WQ_MEM_RECLAIM, 0);
	if (!dax->grow_wq) {
		fxfs_dax_destroy_pools(dax);
		kfree(dax);
		return ERR_PTR(-ENOMEM);
	}
	INIT_WORK(&dax->restage, fxfs_dax_restage_work);
	INIT_LIST_HEAD(&dax->evacuating);
	INIT_WORK(&dax->evacuate, fxfs_dax_evacuate_work);
	dax->restaging = true;
	fxfs_device_set_sealer(dev, fxfs_dax_seal, fxfs_dax_sealed, dax);
	return dax;
}

/*
 * Waits for the regions growing in the background to be as far ahead of the writes so far as
 * they grow, and for the spare to be mapped (fxfs_dax_restage_work): before a sync seals them. A
 * few rounds at most, as writes may go on meanwhile.
 */
void fxfs_dax_settle(struct fxfs_dax *dax)
{
	int round;

	for (round = 0; round < 8 && atomic_read(&dax->growing); round++)
		flush_workqueue(dax->grow_wq);
}

/*
 * Waits for the regions growing in the background, once nothing writes any more: before the
 * filesystem unmounts, or freezes, while its device takes requests. The spare region, whose
 * extent is open, goes, so that the seal after leaves nothing open.
 */
void fxfs_dax_quiesce(struct fxfs_dax *dax)
{
	struct fxfs_dax_region *spare;

	/* No more spares: the filesystem's last seals come as it unmounts, or freezes. */
	WRITE_ONCE(dax->restaging, false);
	if (cancel_work_sync(&dax->restage))
		atomic_dec(&dax->growing);
	cancel_work_sync(&dax->evacuate);
	drain_workqueue(dax->grow_wq);
	fxfs_dax_forget_evacuating(dax);
	spin_lock(&dax->regions_lock);
	spare = dax->spare;
	dax->spare = NULL;
	spin_unlock(&dax->regions_lock);
	if (spare)
		fxfs_dax_put_region(dax, spare);
}

/* Has the next seals map a spare region again, as the filesystem thaws (fxfs_dax_quiesce). */
void fxfs_dax_thaw(struct fxfs_dax *dax)
{
	WRITE_ONCE(dax->restaging, true);
}

/*
 * Frees `dax` once the filesystem is unmounted, its inodes evicted: the window's ranges are
 * unmapped as the device goes.
 */
void fxfs_dax_destroy(struct fxfs_dax *dax)
{
	struct fxfs_dax_range *range, *next;
	struct fxfs_dax_region *region;
	struct fxfs_dax_file *file;
	struct hlist_node *tmp;
	unsigned long ino;
	int bucket;

	/* As fxfs_dax_quiesce, where mounting failed. */
	WRITE_ONCE(dax->restaging, false);
	if (cancel_work_sync(&dax->restage))
		atomic_dec(&dax->growing);
	cancel_work_sync(&dax->evacuate);
	fxfs_dax_forget_evacuating(dax);
	list_for_each_entry_safe(range, next, &dax->used, list) {
		xa_erase(&range->file->ranges, range->chunk);
		list_del(&range->list);
		if (!range->region)
			fxfs_dax_give_window(dax, range->window_offset, range->pages);
		/* Its regions go below. */
		fxfs_dax_free_range_memory(range);
	}
	hash_for_each_safe(dax->regions, bucket, tmp, region, node) {
		hash_del(&region->node);
		fxfs_dax_give_window(dax, region->window_offset, region->pages);
		kfree(region);
	}
	xa_for_each(&dax->files, ino, file) {
		xa_erase(&dax->files, ino);
		fxfs_dax_put_file(dax, file);
	}
	xa_destroy(&dax->files);
	xa_destroy(&dax->shadow);
	destroy_workqueue(dax->grow_wq);
	fxfs_dax_destroy_pools(dax);
	kfree(dax);
}

void fxfs_dax_inode_init(struct fxfs_dax_inode *di)
{
	di->file = NULL;
}

/*
 * The file `ino`, whose ranges outlive its inodes, made if it has none, with `inode` read in for
 * it; NULL without the memory for it.
 */
static struct fxfs_dax_file *fxfs_dax_attach(struct fxfs_dax *dax, struct inode *inode)
{
	struct fxfs_dax_file *file;

	mutex_lock(&dax->files_lock);
	file = xa_load(&dax->files, inode->i_ino);
	if (!file) {
		file = kzalloc(sizeof(*file), GFP_NOFS);
		if (!file)
			goto out;
		file->ino = inode->i_ino;
		refcount_set(&file->refs, 1);
		init_rwsem(&file->sem);
		xa_init(&file->ranges);
		INIT_LIST_HEAD(&file->writable);
		INIT_LIST_HEAD(&file->evacuating);
		if (xa_err(xa_store(&dax->files, inode->i_ino, file, GFP_NOFS))) {
			kfree(file);
			file = NULL;
			goto out;
		}
	}
	WARN_ON_ONCE(file->inode);
	file->inode = inode;
out:
	mutex_unlock(&dax->files_lock);
	return file;
}

void fxfs_dax_set_inode(struct inode *inode)
{
	struct fxfs_dax_file *file = fxfs_dax_attach(fxfs_dax_of(inode), inode);

	/* Without its file, it stays in the page cache. */
	if (!file)
		return;
	fxfs_vfs_inode(inode)->dax.file = file;
	inode->i_flags |= S_DAX;
	inode->i_fop = &fxfs_dax_file_operations;
	inode->i_mapping->a_ops = &fxfs_dax_aops;
}

/*
 * Detaches `inode`, which the kernel evicts, from its file, whose ranges stay mapped for the next
 * inode read in for it: unless no name links it, when they go.
 */
void fxfs_dax_evict(struct inode *inode)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range;
	unsigned long chunk;
	bool forgotten;

	if (!dax || !IS_DAX(inode) || !file)
		return;
	if (!inode->i_nlink) {
		fxfs_dax_lock_unheld(dax, file);
		xa_for_each(&file->ranges, chunk, range)
			fxfs_dax_unmap_range(dax, range);
		up_write(&file->sem);
	}
	mutex_lock(&dax->files_lock);
	file->inode = NULL;
	forgotten = fxfs_dax_forget_file(dax, file);
	mutex_unlock(&dax->files_lock);
	if (forgotten)
		fxfs_dax_put_file(dax, file);
	fxfs_vfs_inode(inode)->dax.file = NULL;
}

int fxfs_dax_shrunk(struct inode *inode, loff_t size)
{
	return fxfs_dax_break_layouts(inode, round_up(size, PAGE_SIZE));
}

int fxfs_dax_changed(struct fxfs_dax *dax, struct inode *inode, u64 ino, loff_t start,
		     loff_t end)
{
	pgoff_t first = start >> FXFS_DAX_SHIFT, last = end >> FXFS_DAX_SHIFT;
	struct fxfs_dax_file *file;
	struct fxfs_dax_range *range;
	unsigned long chunk;
	bool forgotten;
	int err;

	if (inode) {
		if (!IS_DAX(inode) || !fxfs_dax_file(inode))
			return 0;
		return fxfs_dax_drop(inode, first, last, -1);
	}
	for (;;) {
		mutex_lock(&dax->files_lock);
		file = xa_load(&dax->files, ino);
		if (file)
			refcount_inc(&file->refs);
		mutex_unlock(&dax->files_lock);
		if (!file)
			return 0;
		/* As reclaim locks them: the file's semaphore, then the files lock. */
		down_write(&file->sem);
		mutex_lock(&dax->files_lock);
		if (!file->inode)
			break;
		/* An inode read in since: through it, once it is had, or gone. */
		inode = igrab(file->inode);
		mutex_unlock(&dax->files_lock);
		up_write(&file->sem);
		fxfs_dax_put_file(dax, file);
		if (inode) {
			filemap_invalidate_lock(inode->i_mapping);
			err = fxfs_dax_changed(dax, inode, ino, start, end);
			filemap_invalidate_unlock(inode->i_mapping);
			iput(inode);
			return err;
		}
		cond_resched();
	}
	/* No inode: nothing uses its ranges but the window, nor can, the files lock held. */
	xa_for_each_range(&file->ranges, chunk, range, first, last) {
		if (WARN_ON_ONCE(atomic_read(&range->users)))
			continue;
		fxfs_dax_unmap_range(dax, range);
	}
	forgotten = fxfs_dax_forget_file(dax, file);
	mutex_unlock(&dax->files_lock);
	up_write(&file->sem);
	if (forgotten)
		fxfs_dax_put_file(dax, file);
	fxfs_dax_put_file(dax, file);
	return 0;
}

/* Waits for a page in use, without the invalidate_lock, as fuse_wait_dax_page. */
static void fxfs_dax_wait_page(struct inode *inode)
{
	filemap_invalidate_unlock(inode->i_mapping);
	schedule();
	filemap_invalidate_lock(inode->i_mapping);
}

/*
 * Waits for the pages of `inode` from `start` on to be unused, and removes their DAX entries, as
 * fuse_dax_break_layouts: with the invalidate_lock held for writing.
 */
static int fxfs_dax_break_layouts(struct inode *inode, loff_t start)
{
	lockdep_assert_held_write(&inode->i_mapping->invalidate_lock);

	return dax_break_layout(inode, start, LLONG_MAX, fxfs_dax_wait_page);
}

/*
 * Zeroes the `len` bytes of `inode` at `pos`, in one page, through the window: dropping a slice
 * that can't take them where it is, to be mapped again. With the invalidate_lock held for writing.
 */
/*
 * Writes back `inode`'s DAX entries of the pages `len` bytes at `pos` touch, cleaning them, before
 * those pages are zeroed: zeroing may move a page (fxfs_dax_cow), and dax_zero_iter invalidates
 * the entries of a moved page without cleaning them first, which leaves a dirty one, of the page
 * it moved from. With the invalidate_lock held for writing, which keeps faults from dirtying them
 * again.
 */
static int fxfs_dax_clean(struct inode *inode, loff_t pos, loff_t len)
{
	lockdep_assert_held_write(&inode->i_mapping->invalidate_lock);

	return filemap_write_and_wait_range(inode->i_mapping, round_down(pos, PAGE_SIZE),
					    round_up(pos + len, PAGE_SIZE) - 1);
}

static int fxfs_dax_zero_part(struct inode *inode, loff_t pos, loff_t len)
{
	bool zeroed;
	int err = fxfs_dax_clean(inode, pos, len);

	if (err)
		return err;
	for (;;) {
		err = dax_zero_range(inode, pos, len, &zeroed, &fxfs_dax_iomap_ops);
		if (err != -EXDEV)
			return err;
		err = fxfs_dax_drop(inode, pos >> FXFS_DAX_SHIFT, pos >> FXFS_DAX_SHIFT, -1);
		if (err)
			return err;
	}
}

/*
 * Their DAX entries go, once their pages are unused, and the window's ranges of them, to be mapped
 * again as they are after the reflink: a source's blocks are shared from then on, which its ranges
 * may map writably, to be written in place (fxfs_dax_map_extents), as they can't be any more; and a
 * destination's are other blocks. And the destination's ranges too small for its new size.
 */
int fxfs_dax_remap(struct inode *inode, loff_t pos, loff_t len, loff_t size)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	int err;

	if (!IS_DAX(inode) || !fxfs_dax_file(inode) || !len)
		return 0;
	down_write(&file->sem);
	err = fxfs_dax_write_size_locked(inode);
	up_write(&file->sem);
	if (err)
		return err;
	err = fxfs_dax_break_layouts(inode, round_down(pos, PAGE_SIZE));
	if (!err)
		err = fxfs_dax_drop(inode, pos >> FXFS_DAX_SHIFT, (pos + len - 1) >> FXFS_DAX_SHIFT, -1);
	if (!err)
		err = fxfs_dax_fit(inode, size);
	return err;
}

int fxfs_dax_punch_begin(struct inode *inode, loff_t start, loff_t end)
{
	loff_t first = round_up(start, PAGE_SIZE), last = round_down(end, PAGE_SIZE);
	int err;

	filemap_invalidate_lock(inode->i_mapping);
	err = fxfs_dax_break_layouts(inode, round_down(start, PAGE_SIZE));
	if (!err && first > last) {
		err = fxfs_dax_zero_part(inode, start, end - start);
	} else {
		if (!err && start < first)
			err = fxfs_dax_zero_part(inode, start, first - start);
		if (!err && last < end)
			err = fxfs_dax_zero_part(inode, last, end - last);
	}
	/* The chunks of the whole pages freed, mapped again from where their data is, if anywhere. */
	if (!err && first < last)
		err = fxfs_dax_drop(inode, first >> FXFS_DAX_SHIFT, (last - 1) >> FXFS_DAX_SHIFT, -1);
	if (err)
		filemap_invalidate_unlock(inode->i_mapping);
	return err;
}

void fxfs_dax_punch_end(struct inode *inode)
{
	filemap_invalidate_unlock(inode->i_mapping);
}

int fxfs_dax_resize_begin(struct inode *inode, loff_t size)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	bool zeroed;
	int err;

	filemap_invalidate_lock(inode->i_mapping);
	/* An ordinary grow zeroes from Fxfs's end: first record data appended since it. */
	down_write(&file->sem);
	err = fxfs_dax_write_size_locked(inode);
	up_write(&file->sem);
	if (err) {
		filemap_invalidate_unlock(inode->i_mapping);
		return err;
	}
	if (size >= i_size_read(inode)) {
		loff_t old = i_size_read(inode);

		/*
		 * An ordinary grow copies and zeroes the last partial block through the device.
		 * Seal its window stores first, then drop its mapping, which the copy replaces:
		 * otherwise the window can hide a zero-filled replacement until eviction.
		 */
		if (size > old && !PAGE_ALIGNED(old)) {
			err = fxfs_nofs(fxfs_fsync(fxfs_vfs_fs(inode->i_sb)));
			if (!err)
				err = fxfs_dax_break_layouts(inode, round_down(old, PAGE_SIZE));
			if (!err)
				err = fxfs_dax_clean(inode, old, 1);
			if (!err)
				err = fxfs_dax_drop(inode, old >> FXFS_DAX_SHIFT,
						    old >> FXFS_DAX_SHIFT, -1);
			if (err) {
				filemap_invalidate_unlock(inode->i_mapping);
				return err;
			}
		}
		/* The ranges the file's end moves into, mapped as large as they need. */
		err = fxfs_dax_fit(inode, size);
		if (err)
			filemap_invalidate_unlock(inode->i_mapping);
		return err;
	}
	err = fxfs_dax_break_layouts(inode, round_up(size, PAGE_SIZE));
	if (!err)
		err = fxfs_dax_clean(inode, size, 1);
	/*
	 * What the last page holds past the end reads as zeros if the file grows again. A slice
	 * that can't take the zeros where they are is dropped, to be mapped again.
	 */
	while (!err) {
		err = dax_truncate_page(inode, size, &zeroed, &fxfs_dax_iomap_ops);
		if (err != -EXDEV)
			break;
		err = fxfs_dax_drop(inode, size >> FXFS_DAX_SHIFT, size >> FXFS_DAX_SHIFT, -1);
	}
	if (err)
		filemap_invalidate_unlock(inode->i_mapping);
	return err;
}

void fxfs_dax_resize_end(struct inode *inode, loff_t size)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	unsigned long first = DIV_ROUND_UP(size, PAGE_SIZE), page, chunk;
	struct fxfs_dax_range *range;
	int err;

	fxfs_dax_lock_unheld(dax, file);
	xa_for_each_start(&file->ranges, chunk, range, size >> FXFS_DAX_SHIFT) {
		loff_t start = fxfs_dax_chunk_start(range);

		if (start >= round_up(size, PAGE_SIZE)) {
			fxfs_dax_unmap_range(dax, range);
			continue;
		}
		page = first - (start >> PAGE_SHIFT);
		if (page >= range->pages ||
		    find_next_bit(range->mapped, range->pages, page) == range->pages)
			continue;
		/* A slice's pages past the end stay its region's: the slice no longer maps them. */
		err = range->region ? 0 :
		      fxfs_dax_device_unmap(dax, range->window_offset + ((u64)page << PAGE_SHIFT),
					(u64)(range->pages - page) << PAGE_SHIFT);
		if (err)
			pr_warn("fxfs: unmapping the DAX window at %llx failed: %d\n",
				range->window_offset, err);
		bitmap_clear(range->mapped, page, range->pages - page);
		fxfs_dax_clear_writable(range, page, range->pages - page);
		bitmap_clear(range->relocated, page, range->pages - page);
		if (range->moved)
			memset(&range->moved[page], 0, (range->pages - page) * sizeof(*range->moved));
	}
	up_write(&file->sem);
	filemap_invalidate_unlock(inode->i_mapping);
}
