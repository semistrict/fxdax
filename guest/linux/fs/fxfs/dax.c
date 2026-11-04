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
 * Not at once, for a write(2): a MAP of its own for each page took 200 us of a write's 230. Its bytes
 * go through the staging extent's region, already mapped, and the page lags: the window's page maps
 * what it held, until the first use of it maps the chunk's lagging pages together, from where Fxfs
 * has them (fxfs_dax_catch_up). Nothing but the write's copy uses the region's address: the kernel's
 * entries and the user's mappings know the page's alone.
 *
 * fxdax: a write(2)'s move commits after its bytes, as XFS's copy on write does
 * (fxfs_dax_begin_write, TASK-153.5): until then the file has its old blocks, and a write that
 * fails before its bytes are in, or stops short, leaves it as it was, rather than the new blocks'
 * zeros where its data was. It holds the file's semaphore from its iomap to its end, so that no
 * fault maps the file meanwhile, and commits out of the gate, as a commit may wait for a seal. A
 * fault's move commits first: its bytes come with the user's store, as late as it likes.
 *
 * A chunk whose data is one piece of an extent of at most a chunk's size, as small files' are,
 * sharing extents (the allocator's staging extents), is a slice of that extent's region instead:
 * the window's mapping of the whole extent, from its start, which all the files it holds share. So
 * a file written, or read, takes no request of its own, once its extent is mapped: a region of an
 * open extent is mapped writably, further than its blocks are handed out yet, doubling. A region
 * goes with its last slice. A write a slice can't take where it is, as its blocks move, drops the
 * slice (-EXDEV), and the chunk is mapped again.
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
 * fxdax: with fxfs.dax_cow, and a device that copies on write (DAX_COW, backlog doc-13, TASK-164),
 * none of that writes. The window maps sealed extents, and zeros for holes and pages past a file's
 * end, read-only, nothing open: a store to a page (a fault's, a write(2)'s copy) makes it private
 * on the host, the VM's own, holding what it held, at the same address. Pages a write's iomap makes
 * writable join their file's dirty set, with room reserved in Fxfs for their writeback, and count
 * against the device's private_max. Writeback (fsync, sync, writepages, the budget) swaps the set
 * out, the writeback set, write-protects what faults made writable, and has Fxfs write the pages'
 * bytes from the window to new extents, allocated then, committing after them, as the page cache's
 * writeback does (fxfs_dax_cow_write_back). Once the barrier after it has sealed those extents, a
 * private page in neither set may be mapped back to its extent's page, the same bytes at the same
 * address (rebinding), under its file's semaphore held for writing, which sees no store to it
 * under way. Nothing moves, so nothing seals but what Fxfs commits; files created in a mount are
 * page-cache inodes until read in again (fxfs_dax_set_inode). The gate, seals' write-protects,
 * moves, staging and lagging pages are the writable window's alone.
 *
 * Locks are taken in this order: an inode's lock (read and write); fxdax: a follower's fill
 * barrier (vfs.h), which a batch takes holding no inode's lock; an inode's invalidate_lock
 * (faults, truncation and reclaim); fxdax: copying on write, its file's writeback lock
 * (`wb_lock`); then its file's DAX semaphore, then the gate. Another inode's
 * are only tried, when reclaiming its ranges. Rust is called holding no gate. fxdax: a write(2)
 * moving its blocks holds its file's semaphore from its iomap to its end, across its copy, and
 * with it the Fxfs transaction of the move, which commits after the bytes (fxfs_dax_begin_write).
 */

#include <linux/dax.h>
#include <linux/completion.h>
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
#include <linux/vmalloc.h>
#include <linux/virtio_extent.h>
#include <linux/wait.h>
#include <linux/wait_bit.h>
#include <linux/workqueue.h>
#include <linux/writeback.h>

#include "device.h"
#include "hist.h"
#include "vfs.h"

/* A range's size: a power of two, the window's and chunks' alignment. */
#define FXFS_DAX_SHIFT 21
#define FXFS_DAX_SIZE (1ULL << FXFS_DAX_SHIFT)
#define FXFS_DAX_PAGES (FXFS_DAX_SIZE >> PAGE_SHIFT)
/*
 * fxdax: copying on write, the most pages one write's iomap makes private: an eighth of the
 * device's private_max, if less (fxfs_dax_cow_iomap_write).
 */
#define FXFS_DAX_COW_PAGES 64UL
/* fxdax: a sustained, unsealed append stream reserves this many bytes at a time (TASK-92.58). */
#define FXFS_DAX_APPEND_SIZE (16UL << PAGE_SHIFT)

/*
 * The window's space is handed out from a pool, page by page, at this offset: the pool takes no
 * address 0.
 */
#define FXFS_DAX_POOL_BASE (1UL << 48)

/*
 * The most pages a writable region's mapping grows by at once. fxdax: a MAP in a write holds the
 * inode's lock, which the file's other writes queue for (TASK-146.2).
 */
static unsigned long fxfs_dax_region_step = 64;
module_param_named(dax_region_step, fxfs_dax_region_step, ulong, 0644);
MODULE_PARM_DESC(dax_region_step, "The most pages a writable region's mapping grows by at once");
/* fxdax: how many waits of 10 ms with nothing let go of a range waits for room for (30 s). */
#define FXFS_DAX_NO_ROOM_WAITS 3000
/* fxdax: how many times a running write or fault has a chunk's range dropped before giving up. */
#define FXFS_DAX_MAX_DROPS 256
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

/* fxdax: whether a fault injection counting down in `left` injects once more, counting it. */
static bool fxfs_dax_inject(unsigned int *left)
{
	unsigned int now = READ_ONCE(*left);

	while (now) {
		unsigned int was = cmpxchg(left, now, now - 1);

		if (was == now)
			return true;
		now = was;
	}
	return false;
}

/* fxdax: whether to seal after a fault's move, as fxfs.dax_fault_seals_moved asks, counting it. */
static bool fxfs_dax_seal_moved(void)
{
	return fxfs_dax_inject(&fxfs_dax_fault_seals_moved);
}

/*
 * fxdax: fault injection: of the next this many writes whose blocks Fxfs moved into more than one
 * piece, the range maps the first piece alone, then fails as if the device's MAP of the next had,
 * the others mapping the blocks where they were, which Fxfs freed (TASK-79.106).
 */
static bool fxfs_dax_fail_attach;
module_param_named(dax_fail_attach, fxfs_dax_fail_attach, bool, 0644);
MODULE_PARM_DESC(dax_fail_attach, "Fail inodes' attaching to their files, which leaves them in the page cache (fault injection)");

/*
 * fxdax: copying on write, of the next this many maps of a range's pages in more than one piece,
 * the range maps the first piece alone, then fails (fxfs_dax_cow_map_all).
 */
static unsigned int fxfs_dax_fail_maps;
module_param_named(dax_fail_maps, fxfs_dax_fail_maps, uint, 0644);
MODULE_PARM_DESC(dax_fail_maps, "Fail the MAP of the second piece of the next this many writes whose blocks moved, or, copying on write, ranges' maps of more than one piece (fault injection)");

/*
 * fxdax: fault injection of a chunk's map failing part way (TASK-119.13): a MAP_MANY that maps its
 * first member, then fails, as a device's may; and the MAP, one by one, of the second piece on
 * after it.
 */
static unsigned int fxfs_dax_fail_map_many;
module_param_named(dax_fail_map_many, fxfs_dax_fail_map_many, uint, 0644);
MODULE_PARM_DESC(dax_fail_map_many, "Fail the next this many MAP_MANYs after their first member (fault injection)");

static unsigned int fxfs_dax_fail_fallbacks;
module_param_named(dax_fail_fallbacks, fxfs_dax_fail_fallbacks, uint, 0644);
MODULE_PARM_DESC(dax_fail_fallbacks, "Fail the next this many MAPs, after a failed MAP_MANY, of pieces after its first (fault injection)");

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
 * fxdax: diagnostic: whether a write(2)'s bytes go through the staging extent's region, its pages
 * lagging (fxfs_dax_catch_up), rather than each mapped in its range's own space: off, a failing
 * check says whether this path is the one losing data.
 */
static bool fxfs_dax_writes_through = true;
module_param_named(dax_writes_through, fxfs_dax_writes_through, bool, 0644);
MODULE_PARM_DESC(dax_writes_through, "Write through staging extents' regions, mapping pages together later (diagnostic)");
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

/* Fxfs checks the blocks it keeps as pages move with it (fxfs_set_check_kept). */
static int fxfs_dax_set_check_moves(const char *val, const struct kernel_param *kp)
{
	int err = param_set_bool(val, kp);

	if (!err)
		fxfs_set_check_kept(fxfs_dax_check_moves);
	return err;
}

static const struct kernel_param_ops fxfs_dax_check_moves_ops = {
	.set = fxfs_dax_set_check_moves,
	.get = param_get_bool,
};
module_param_cb(dax_check_moves, &fxfs_dax_check_moves_ops, &fxfs_dax_check_moves, 0644);
MODULE_PARM_DESC(dax_check_moves, "WARN of a page moved holding other data (diagnostic)");
/*
 * fxdax: diagnostic (TASK-121): with the shadow, as each seal begins, the pages the guest mapped
 * writably since the seal before are read through the window and from the device, what the seal's
 * commit is about to write, and where they differ logged: stores the window took that the extent
 * doesn't hold (fxfs_dax_verify_writable). fxdax: it read the regions of staging extents sealed
 * after the seal too, which writes may have emptied and Fxfs freed meanwhile, the window mapping
 * zeros or nothing there: false findings, and accesses the host refused, which
 * stopped a VM (TASK-119.21).
 */
static bool fxfs_dax_verify_seals;
module_param_named(dax_verify_seals, fxfs_dax_verify_seals, bool, 0644);
MODULE_PARM_DESC(dax_verify_seals, "Compare pages written since the last seal through the window with the device, as a seal begins, with dax_shadow (diagnostic)");

/*
 * fxdax: a diagnostic found the window, or the guest's view of it, wrong: WARNs, once, so that a
 * test with diagnostics on fails, as the runner fails a test whose kernel WARNs (TASK-44.8), the
 * diagnostic's own lines saying what it found. They logged alone, and a test passed on them
 * (TASK-119.3).
 */
static void fxfs_dax_diagnosed(void)
{
	WARN_ONCE(1, "fxfs: a DAX diagnostic found the window wrong\n");
}

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
	/* A read of a file, its iomap made, before the copy of what it maps, holding its range. */
	FXFS_DAX_STEP_READ_MAPPED,
	/* A write of a file, its iomap made, in the gate, before the copy into what it maps. */
	FXFS_DAX_STEP_WRITE_MAPPED,
	/* A fault whose blocks moved, the gate open but a seal ended since, before it gives up. */
	FXFS_DAX_STEP_RESEALED,
	/*
	 * fxdax: copying on write, a rebind about to MAP a run of a file's clean private pages over,
	 * holding the file's writeback lock and its semaphore for writing (fxfs_dax_cow_rebind).
	 */
	FXFS_DAX_STEP_REBIND,
	/*
	 * fxdax: copying on write, a writeback, its dirty set swapped out, before it reads the set's
	 * pages from the window, holding their ranges and the file's writeback lock
	 * (fxfs_dax_cow_write_back).
	 */
	FXFS_DAX_STEP_WRITEBACK,
	/*
	 * fxdax: copying on write, a write fault past its iomap_end, its PTE writable and the file's
	 * semaphore let go, holding its page's DAX entry until it returns (fxfs_dax_cow_protect).
	 */
	FXFS_DAX_STEP_FAULTED,
};
static unsigned long fxfs_dax_pause_steps;
module_param_named(dax_pause, fxfs_dax_pause_steps, ulong, 0644);
MODULE_PARM_DESC(dax_pause, "Steps (bits) of the DAX protocol tasks wait at, until cleared (pause points)");
static unsigned long fxfs_dax_paused_steps;
module_param_named(dax_paused, fxfs_dax_paused_steps, ulong, 0444);
MODULE_PARM_DESC(dax_paused, "Steps (bits) a task waits at (pause points)");
/* fxdax: the file whose tasks wait at the steps of a file, any file's if 0 (pause points). */
static unsigned long fxfs_dax_pause_ino;
module_param_named(dax_pause_ino, fxfs_dax_pause_ino, ulong, 0644);
MODULE_PARM_DESC(dax_pause_ino, "The inode whose tasks wait at fxfs.dax_pause's steps of a file, or 0 for any (pause points)");

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

/* fxdax: waits at `step`, of file `ino`, as fxfs_dax_pause, if fxfs.dax_pause_ino names it. */
static void fxfs_dax_pause_file(enum fxfs_dax_step step, u64 ino)
{
	unsigned long only = READ_ONCE(fxfs_dax_pause_ino);

	if (!only || only == ino)
		fxfs_dax_pause(step);
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
	/* Its slices, and its growth. Guarded by the regions lock. */
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
};

/* A file's ranges of the window, which outlive its inodes. */
/* A mounted volume's share of the window: the files of its inode numbers, which are its own. */
struct fxfs_dax_volume {
	struct fxfs *fs;
	/* The files with ranges, or inodes using the window, by inode number. */
	struct xarray files;
	/* On the filesystem's volumes. Guarded by its files lock. */
	struct list_head list;
	/*
	 * fxdax: copying on write, the page-cache inodes whose writeback wrote since the last seal
	 * began, by inode number, each the seal count at which a barrier will have sealed it
	 * (fxfs_dax_written, fxfs_dax_seal_target): read in before then, an inode is a page-cache
	 * inode again, as the window would find its extents open, which it maps none of (doc-13,
	 * TASK-164.3). The worker prunes it as seals come.
	 */
	struct xarray unsealed;
};

struct fxfs_dax_file {
	/* Its volume, until the volume goes (fxfs_dax_remove_volume). Guarded by the files lock. */
	struct fxfs_dax_volume *volume;
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
	 * fxdax: the region the write under way copies through, which it holds a reference to, for its
	 * iomap's length: under the inode's write lock.
	 */
	struct fxfs_dax_region *through;
	/*
	 * fxdax: a write a slice refused twice, under the inode's write lock: its chunk gets a range
	 * of its own, so that no slice can refuse it again (xfstests generic/638).
	 */
	bool no_slices;
	/*
	 * fxdax: how far a write past the end grows the file, under the inode's write lock, which
	 * the ranges mapped meanwhile are to reach: a fault on the write's own source, a mapping of
	 * the file, mapped the chunk only as large as the file, again each time the write dropped it
	 * for being too small, losing the write's end (xfstests generic/638, TASK-147).
	 */
	loff_t growing;
	/*
	 * fxdax: the move of the write(2) under way, from its iomap to its end, which commits it once
	 * its bytes are in, holding the file's semaphore for writing all along (TASK-153.5).
	 */
	struct fxfs_dax_write *pending;
	/*
	 * fxdax: where the write under way's bytes stop being the file's, if an iomap's move was
	 * dropped, or failed to commit, once dax_iomap_rw had counted them, and why (0 if none):
	 * under the inode's write lock (fxfs_dax_committed).
	 */
	loff_t unwritten;
	int unwritten_err;
	/*
	 * fxdax: the chunk whose slice the write under way left behind its blocks, which went through
	 * another region, plus one, or 0: under the inode's write lock (fxfs_dax_follow_slice).
	 */
	pgoff_t slice_left;
	/*
	 * fxdax: copying on write (doc-13): held by the file's writeback, from before it swaps the
	 * dirty set out until Fxfs has committed the pages it wrote, which rebinding and truncation
	 * wait for: a writeback's pages, its writeback set, are those it holds it for. Taken before
	 * the semaphore.
	 */
	struct mutex wb_lock;
	/* Its ranges' pages in its dirty set, and counted private (fxfs_dax_cow_count). */
	atomic_long_t dirty_pages, private_pages;
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
	/*
	 * fxdax: the faults past their iomaps of it, until their iomap_end, which may yet map its pages
	 * holding no lock of the file's: a move of its blocks waits for them (fxfs_dax_iomap_write).
	 * Raised under its file's semaphore.
	 */
	atomic_t faulting;
	/* Its pages that map an extent, and those that map one writably. */
	unsigned long *mapped;
	unsigned long *writable;
	/*
	 * fxdax: its pages whose blocks moved, written through a region, which the window doesn't map
	 * where they are yet (fxfs_dax_catch_up): whatever `mapped` says, they hold nothing to read.
	 * Under its file's semaphore.
	 */
	unsigned long *lagging;
	/*
	 * fxdax: if Fxfs moved blocks it maps where they were, which it failed to follow, what its
	 * iomaps say: nothing uses it from then on, but to drop it (fxfs_dax_fall_behind). Set and read
	 * under its file's semaphore.
	 */
	int behind;
	/*
	 * fxdax: copying on write (doc-13), each NULL otherwise: its pages counted private, from the
	 * write's iomap that made them writable to the MAP or UNMAP over them; in its file's dirty
	 * set; and mapped as zeros, not a file's data, as holes and pages past its end are. Set and
	 * cleared under its file's semaphore, held shared to set `private` and `dirty` (atomically),
	 * and for writing to clear them.
	 */
	unsigned long *private;
	unsigned long *dirty;
	unsigned long *zeros;
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
 * TASK-112.1).
 */
enum fxfs_dax_why {
	FXFS_DAX_REACH,
	FXFS_DAX_GROW,
	FXFS_DAX_RESTAGE,
	FXFS_DAX_RANGE,
	FXFS_DAX_WHYS,
};

static const char *const fxfs_dax_accesses[] = {
	[FXFS_DAX_SEALED] = "sealed", [FXFS_DAX_OPEN] = "open", [FXFS_DAX_SHARED] = "shared",
};

static const char *const fxfs_dax_whys[FXFS_DAX_WHYS] = {
	"reach", "grow", "restage", "range",
};

struct fxfs_dax {
	struct fxfs_filesystem *filesystem;
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
	/* fxdax: the window's size, which no page's place is past (fxfs_dax_page_offset). */
	u64 window_len;
	/*
	 * fxdax: how often the window had no room for a range, or for a region in their own space,
	 * how many ranges were reclaimed for room, and how many regions there are (TASK-91.7).
	 */
	atomic_long_t window_full, regions_full, reclaims, region_count;
	/* fxdax: the window's MAPs, and the bytes they mapped, by why (`enum fxfs_dax_why`). */
	atomic64_t maps[FXFS_DAX_WHYS], map_bytes[FXFS_DAX_WHYS];
	/* fxdax: the writes that made room through a Fxfs transaction (TASK-92.28). */
	atomic64_t map_writes;
	/* fxdax: the writes through regions, the catch-ups after them, and the pages those mapped. */
	atomic64_t writes_through, catch_ups, caught_up;
	/* fxdax: the window's bytes kept from use as their UNMAP failed (TASK-44.22). */
	atomic64_t lost_bytes;
	/* fxdax: the faults whose iomap mapped a PMD's 2 MiB, its pages in a row (TASK-123). */
	atomic64_t huge_maps;
	/* Requested bytes of extent-result buffers, before allocator rounding. */
	atomic64_t extent_alloc_bytes;
	/*
	 * fxdax: the writes that waited for their file's semaphore, and how long, in ns (TASK-91.16).
	 */
	atomic64_t write_waits, write_wait_ns;
	/*
	 * fxdax: where write(2)'s time goes (TASK-146.2): in all, waiting for the inode's lock, for
	 * the gate, in Fxfs making room, and mapping what moved.
	 */
	atomic64_t writes, write_ns, write_lock_ns, write_gate_ns, write_fxfs_ns, write_map_ns;
	/*
	 * fxdax: where else a write(2)'s time goes, so that write_ns adds up: what comes before its
	 * first iomap, faulting in its source, dax_iomap_rw (of which its iomaps' begin and end, the
	 * rest being the copy, a store's EPT faults on the window among it), dropping a range to try
	 * again, and what comes after (TASK-146.2).
	 */
	atomic64_t write_setup_ns, write_fault_in_ns, write_rw_ns, write_begin_ns, write_end_ns;
	atomic64_t write_retry_ns, write_tail_ns, write_prepare_ns, write_range_ns;
	/* Of write_range_ns, what it spent waiting for the file's semaphore. */
	atomic64_t write_range_wait_ns;
	struct fxfs_hist write_hist;
	/* fxdax: what each fault of a mapping took (TASK-111.6). */
	struct fxfs_hist fault_hist;
	/*
	 * fxdax: the writes whose blocks Fxfs moved before a seal kept them from the gate, which
	 * their ranges map read-only where they moved to (fxfs_dax_map_moved, TASK-121).
	 */
	atomic64_t moved_unsealed;
	/* fxdax: write(2)s' moves dropped, uncommitted: a seal, a failure, a copy cut short. */
	atomic64_t writes_dropped;
	/* fxdax: MAPs the host had no memory for, and seals they had made (fxfs_dax_make_memory). */
	atomic64_t maps_refused;
	atomic64_t memory_seals;
	/* The volumes mounted, each with its files. */
	struct list_head volumes;
	/*
	 * fxdax: with fxfs.dax_shadow, the window's pages as the guest mapped them last, by page:
	 * the device's seal count then, shifted, and whether writably (TASK-121).
	 */
	struct xarray shadow;
	/*
	 * fxdax: with the shadow, what each window page maps: its extent's sequence, shifted 20 bits,
	 * and its page in the extent (fxfs.dax_verify_seals, TASK-121).
	 */
	struct xarray shadow_extent;
	/* fxdax: the branch of the extents the window maps, the device's (TASK-121). */
	u64 shadow_branch;
	/* Guards `volumes`, their files, and files' inodes. */
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
	/* Whether seals restage: until the filesystem unmounts. */
	bool restaging;
	/*
	 * fxdax: whether the window copies on write (doc-13, fxfs.dax_cow): the device took DAX_COW.
	 * Then the most pages it holds private at once, which the guest keeps under, and the pages
	 * counted private, and in files' dirty sets (fxfs_dax_cow_count).
	 */
	bool cow;
	long private_max;
	/* fxdax: the most pages one write's iomap makes private: a slice of the budget. */
	unsigned long cow_pages;
	atomic_long_t private, dirty;
	/*
	 * fxdax: the budget's worker (fxfs_dax_cow_work): writes back dirty files, and rebinds clean
	 * private pages, as the private pages near private_max, the window runs out of room, or a
	 * test asks; its rounds, which those it throttles wait for (`cow_wait`); whether it is to
	 * write back whatever is dirty (`cow_clean`) and rebind everything it can (`cow_rebind_all`),
	 * as asked since its last round; and whether it stops, the filesystem unmounting or freezing,
	 * and whether it is frozen, when it writes nothing.
	 */
	struct work_struct cow_work;
	wait_queue_head_t cow_wait;
	atomic64_t cow_rounds;
	atomic_t cow_clean, cow_rebind_all;
	bool cow_stopping, frozen;
	/* fxdax: the page-cache inodes in the volumes' `unsealed`, which the worker prunes. */
	atomic_long_t unsealed;
	/*
	 * fxdax: the writebacks, the pages they wrote, the pages rebound, the iomaps the budget held
	 * up, the seals made for a range or an UNMAP, the PRIVATE requests of writes, and those of
	 * writebacks (fxfs_dax_cow_own) (TASK-164.2).
	 */
	atomic64_t cow_writebacks, cow_written, cow_rebound, cow_throttled, cow_seals, cow_privates;
	atomic64_t cow_owns;
	/*
	 * fxdax: MAPs the device refused as of an open extent (-EPERM), which Fxfs had taken for
	 * sealed: none is meant to be asked for (doc-13's invariant 1), a tripwire the checks read.
	 */
	atomic64_t cow_open_maps;
};

/* Each of `dax`'s files, of each volume, with its files lock held. */
#define fxfs_dax_for_each_file(dax, volume, ino, file)			\
	list_for_each_entry(volume, &(dax)->volumes, list)		\
		xa_for_each(&(volume)->files, ino, file)

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
				bool mapped, u64 sequence, u64 offset)
{
	u64 seals = fxfs_device_seals(dax->dev);
	unsigned long page;

	if (!READ_ONCE(fxfs_dax_shadowed))
		return;
	for (page = 0; mapped && page < len >> PAGE_SHIFT; page++)
		xa_store(&dax->shadow_extent, (window_offset >> PAGE_SHIFT) + page,
			 xa_mk_value((sequence << 20) | ((offset >> PAGE_SHIFT) + page)), GFP_NOFS);
	for (page = window_offset >> PAGE_SHIFT; page < (window_offset + len) >> PAGE_SHIFT; page++) {
		void *entry = xa_load(&dax->shadow, page);
		u64 then = entry ? xa_to_value(entry) >> 1 : 0;

		/*
		 * A page mapped writably since the last seal, or before one begun and waiting, which
		 * the user's mappings may map writably still: mapped read-only, or unmapped, their
		 * stores are refused.
		 */
		if (mapped && !writable && entry && (xa_to_value(entry) & 1) &&
		    (then == seals || ((seals & 1) && then == seals - 1))) {
			pr_err("fxfs: window page %lx, mapped writably at seals %llu, mapped read-only at %llu by %pS\n",
			       page, then, seals, __builtin_return_address(1));
			fxfs_dax_diagnosed();
		}
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

	if (!err) {
		WRITE_ONCE(dax->shadow_branch, branch);
		fxfs_dax_shadow_map(dax, window_offset, len, writable, true, sequence, offset);
	} else if (err == -ENOMEM) {
		atomic64_inc(&dax->maps_refused);
	}
	return err;
}

static int fxfs_dax_device_map_many(struct fxfs_dax *dax, const struct fxfs_device_map *maps,
				    unsigned int count)
{
	unsigned int i;
	int err;

	if (count > 1 && fxfs_dax_inject(&fxfs_dax_fail_map_many)) {
		err = fxfs_dax_device_map(dax, maps[0].branch, maps[0].sequence, maps[0].offset,
					  maps[0].len, maps[0].window_offset, maps[0].writable,
					  maps[0].ahead);
		return err ?: -EIO;
	}
	err = fxfs_device_map_many(dax->dev, maps, count);
	if (err == -ENOMEM)
		atomic64_inc(&dax->maps_refused);
	for (i = 0; !err && i < count; i++)
		fxfs_dax_shadow_map(dax, maps[i].window_offset, maps[i].len, maps[i].writable, true,
				    maps[i].sequence, maps[i].offset);
	return err;
}

static int fxfs_dax_device_unmap(struct fxfs_dax *dax, u64 window_offset, u64 len)
{
	int err = fxfs_device_unmap(dax->dev, window_offset, len);

	fxfs_dax_shadow_map(dax, window_offset, len, false, false, 0, 0);
	return err;
}

struct fxfs_dax_read_wait {
	struct completion done;
	int status;
};

static void fxfs_dax_read_done(void *arg, int status)
{
	struct fxfs_dax_read_wait *wait = arg;

	wait->status = status;
	complete(&wait->done);
}

/* fxdax: diagnostic: reads the page at `offset` in an extent from the device into `buf`. */
static int fxfs_dax_read_device_page(struct fxfs_dax *dax, u64 branch, u64 sequence, u64 offset,
				     void *buf)
{
	struct fxfs_device_op op = {
		.type = VIRTIO_EXTENT_T_READ, .branch = branch, .sequence = sequence,
		.offset = offset, .data = buf, .len = PAGE_SIZE,
	};
	struct fxfs_dax_read_wait wait;
	int err;

	init_completion(&wait.done);
	err = fxfs_device_submit(dax->dev, &op, fxfs_dax_read_done, &wait);
	if (err)
		return err;
	wait_for_completion(&wait.done);
	return wait.status;
}

/*
 * fxdax: diagnostic: reads from the device the extent's page the shadow has the window map at
 * window page `page` into `buf`, and returns where it is, (sequence << 20) | page, or -ENOENT.
 */
static s64 fxfs_dax_read_shadowed(struct fxfs_dax *dax, unsigned long page, void *buf)
{
	void *entry = xa_load(&dax->shadow_extent, page);
	u64 where;
	int err;

	if (!entry)
		return -ENOENT;
	where = xa_to_value(entry);
	err = fxfs_dax_read_device_page(dax, READ_ONCE(dax->shadow_branch), where >> 20,
					(where & ((1 << 20) - 1)) << PAGE_SHIFT, buf);
	return err ? err : where;
}

/*
 * fxdax: diagnostic: how many of the first `limit` bytes pages `a` and `b` differ in, but for the
 * `len` bytes from `skip`, and in `first`, the first, or PAGE_SIZE.
 */
static size_t fxfs_dax_differ(const u8 *a, const u8 *b, size_t skip, size_t len, size_t *first,
			      size_t limit)
{
	size_t i, differ = 0;

	*first = PAGE_SIZE;
	for (i = 0; i < limit; i++) {
		if ((i < skip || i >= skip + len) && a[i] != b[i]) {
			*first = min(*first, i);
			differ++;
		}
	}
	return differ;
}

/* Where `range`'s page `page` is in the window, whatever extent holds it. */
static u64 fxfs_dax_page_offset(const struct fxfs_dax_range *range, unsigned long page)
{
	return range->window_offset + ((u64)page << PAGE_SHIFT);
}

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
		     range->region ? "a slice of region" : "in its own space at",
		     range->region ? range->region->window_offset : range->window_offset);
	}
}
static const struct address_space_operations fxfs_dax_aops;
static const struct file_operations fxfs_dax_file_operations;
static int fxfs_dax_break_layouts(struct inode *inode, loff_t start);
static int fxfs_dax_zero_tail(struct inode *inode, loff_t old, loff_t size);
static int fxfs_dax_reach(struct fxfs_dax *dax, struct fxfs_dax_region *region, unsigned long end);
static void fxfs_dax_put_region(struct fxfs_dax *dax, struct fxfs_dax_region *region);
static void fxfs_dax_grow(struct work_struct *work);
static void fxfs_dax_restage_work(struct work_struct *work);
static int fxfs_dax_cow_write_back(struct inode *inode);

static struct fxfs_dax *fxfs_dax_of(struct inode *inode)
{
	return fxfs_vfs_info(inode->i_sb)->dax;
}

static struct fxfs_dax_volume *fxfs_dax_volume_of(struct inode *inode)
{
	return fxfs_vfs_info(inode->i_sb)->dax_volume;
}

/* The file `inode` is read in for, whose ranges it uses. */
static struct fxfs_dax_file *fxfs_dax_file(struct inode *inode)
{
	return fxfs_vfs_inode(inode)->dax.file;
}

static loff_t fxfs_dax_chunk_start(const struct fxfs_dax_range *range)
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

/*
 * Has `range`'s `pages` pages from `page` writable, and its file among those with writable pages in
 * the window, which the sealer forgets: the one way a page gets to be, as a page writable of a file
 * not listed stays so past a seal, and a store to it is refused. fxdax: a slice refusing a write's
 * second piece had set the first's bits, and left before it listed the file (TASK-150.14). In the
 * gate.
 */
static void fxfs_dax_set_writable(struct fxfs_dax *dax, struct fxfs_dax_range *range,
				  unsigned long page, unsigned long pages)
{
	struct fxfs_dax_file *file = range->file;

	fxfs_device_assert_in_gate(dax->dev);

	mutex_lock(&dax->writable_lock);
	if (list_empty(&file->writable))
		list_add_tail(&file->writable, &dax->writable);
	mutex_unlock(&dax->writable_lock);
	bitmap_set(range->writable, page, pages);
}

/*
 * Takes the pieces `extents` of the chunk the slice `range` maps where it maps them, which a write
 * found open, writably, its region mapped as far as they reach; or -EXDEV if they are elsewhere, as
 * moved blocks are, which the slice can't map. In the gate, with the file's DAX semaphore held for
 * writing.
 */
static int fxfs_dax_map_slice(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			      const struct fxfs_dax_extent *extents, ssize_t count)
{
	struct fxfs_dax_region *region = range->region;
	loff_t start = fxfs_dax_chunk_start(range);
	/* Where the slice starts in its extent. */
	u64 base = range->window_offset - region->window_offset;
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
		    e->extent_offset != base + (e->offset - start) || page + pages > range->pages)
			return -EXDEV;
		err = fxfs_dax_reach(dax, region, end >> PAGE_SHIFT);
		if (err)
			return err;
		fxfs_dax_set_writable(dax, range, page, pages);
	}
	return 0;
}

/*
 * fxdax: diagnostic (TASK-121): with fxfs.dax_trace set as the filesystem mounts, the latest of
 * what happened to files' pages, which writing `ino page` to fxfs.dax_trace_dump logs for that
 * page: where Fxfs had its blocks, where the window maps it, the iomaps of it, and the seals.
 */
enum fxfs_dax_trace_kind {
	FXFS_DAX_TRACE_MAPPED,
	FXFS_DAX_TRACE_WRITE,
	FXFS_DAX_TRACE_FAULT,
	FXFS_DAX_TRACE_READ,
	FXFS_DAX_TRACE_SEAL,
};

static const char *const fxfs_dax_trace_kinds[] = {
	"mapped", "write iomap", "fault iomap", "read iomap", "seal",
};

struct fxfs_dax_trace_event {
	u64 time, seals, ino, page, window, extent, extent_offset;
	u32 kind, flags;
};

#define FXFS_DAX_TRACE_EVENTS (1 << 18)

static bool fxfs_dax_tracing;
module_param_named(dax_trace, fxfs_dax_tracing, bool, 0444);
MODULE_PARM_DESC(dax_trace, "Trace what happens to DAX pages (diagnostic), as the filesystem mounts");
static struct fxfs_dax_trace_event *fxfs_dax_trace_events;
static atomic64_t fxfs_dax_trace_next;
/* The device of the mount traced, the latest: it goes as the mount does (fxfs_dax_destroy). */
static struct fxfs_device *fxfs_dax_trace_dev;
/* The window's first page frame, which `pfn N` written to fxfs.dax_trace_dump is counted from. */
static unsigned long fxfs_dax_trace_base;

static void fxfs_dax_trace(u32 kind, u64 ino, u64 page, u64 window, u64 extent, u64 extent_offset,
			   u32 flags)
{
	struct fxfs_dax_trace_event *events = READ_ONCE(fxfs_dax_trace_events);
	struct fxfs_device *dev = READ_ONCE(fxfs_dax_trace_dev);
	u64 at;

	if (likely(!events) || !dev)
		return;
	at = atomic64_inc_return(&fxfs_dax_trace_next) - 1;
	events[at % FXFS_DAX_TRACE_EVENTS] = (struct fxfs_dax_trace_event) {
		.time = ktime_get_ns(), .seals = fxfs_device_seals(dev), .ino = ino,
		.page = page, .window = window, .extent = extent, .extent_offset = extent_offset,
		.kind = kind, .flags = flags,
	};
}

/* Logs the traced events of `ino`'s page `page`, oldest first: `ino page` written. */
static int fxfs_dax_trace_dump(const char *val, const struct kernel_param *kp)
{
	struct fxfs_dax_trace_event *events = READ_ONCE(fxfs_dax_trace_events), *e;
	u64 ino = 0, page = 0, pfn, window = 0, next, at, first;
	unsigned int shown = 0;
	bool by_window;

	if (!events)
		return -EINVAL;
	/* `pfn N`: the events of the window's page at page frame N, whichever page they were of. */
	by_window = sscanf(val, "pfn %llx", &pfn) == 1;
	if (by_window) {
		if (!fxfs_dax_trace_base || pfn < fxfs_dax_trace_base)
			return -EINVAL;
		window = (pfn - fxfs_dax_trace_base) << PAGE_SHIFT;
	} else if (sscanf(val, "%llu %llu", &ino, &page) != 2) {
		return -EINVAL;
	}
	next = atomic64_read(&fxfs_dax_trace_next);
	first = next > FXFS_DAX_TRACE_EVENTS ? next - FXFS_DAX_TRACE_EVENTS : 0;
	if (by_window)
		pr_err("fxfs: the trace of window page %llx, of %llu events:\n", window, next - first);
	else
		pr_err("fxfs: the trace of ino %llu page %llu, of %llu events:\n", ino, page,
		       next - first);
	for (at = first; at < next; at++) {
		e = &events[at % FXFS_DAX_TRACE_EVENTS];
		if (e->kind == FXFS_DAX_TRACE_SEAL)
			continue;
		if (by_window ? e->window != window : e->ino != ino || e->page != page)
			continue;
		pr_err("fxfs:   %llu ns seals %llu: ino %llu page %llu: %s window %llx extent %llu at %llx flags %x\n",
		       e->time, e->seals, e->ino, e->page, fxfs_dax_trace_kinds[e->kind], e->window,
		       e->extent, e->extent_offset, e->flags);
		if (++shown == 200)
			break;
	}
	return 0;
}

static const struct kernel_param_ops fxfs_dax_trace_dump_ops = { .set = fxfs_dax_trace_dump };
module_param_cb(dax_trace_dump, &fxfs_dax_trace_dump_ops, NULL, 0200);
MODULE_PARM_DESC(dax_trace_dump, "Log the traced events of `ino page`, or `pfn N` (hex), the window's page at page frame N (diagnostic)");

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
 * fxdax: has `range` map its `pages` pages from `page` on no longer, holes, in its own space. The
 * user's mappings of them go now (TASK-125): one left read what the window maps there no more, and
 * held the page's folio as its DAX entry went. With the file's DAX semaphore held for writing, and
 * its inode live: an iomap's.
 */
static void fxfs_dax_unmap_pages(struct fxfs_dax_range *range, unsigned long page,
				 unsigned long pages)
{
	struct inode *inode = READ_ONCE(range->file->inode);

	if (inode)
		unmap_mapping_pages(inode->i_mapping,
				    (fxfs_dax_chunk_start(range) >> PAGE_SHIFT) + page, pages, false);
	fxfs_dax_clear_writable(range, page, pages);
	bitmap_clear(range->mapped, page, pages);
	bitmap_clear(range->lagging, page, pages);
}

/* fxdax: `range`'s pages lagging still, for which Fxfs found no blocks, holes. */
static void fxfs_dax_unlag_holes(struct fxfs_dax_range *range)
{
	unsigned long page;

	for_each_set_bit(page, range->lagging, range->pages)
		fxfs_dax_unmap_pages(range, page, 1);
}

/*
 * fxdax: has `range` say piece `e` of its chunk is mapped, as mapping it did (`err`): writably if
 * it is open. A sealed extent the device doesn't hold was never written: its blocks read as zeros,
 * a hole, as Fxfs reads them. (Blocks a write allocated before a seal sealed their extent
 * unwritten, which the write then moves.) The error, if any.
 */
static int fxfs_dax_mapped(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			   const struct fxfs_dax_extent *e, int err)
{
	loff_t start = fxfs_dax_chunk_start(range);
	unsigned long page = (e->offset - start) >> PAGE_SHIFT;
	unsigned long pages = e->len >> PAGE_SHIFT;

	if (err == -ENOENT && e->access != FXFS_DAX_OPEN)
		return 0;
	if (err) {
		/* fxdax: the host short of memory is no failure yet (fxfs_dax_make_memory). */
		if (err != -ENOMEM)
			pr_warn("fxfs: mapping %llu bytes at %llu of extent %llx:%llu, %s, at %llu of the DAX window failed: %d\n",
				e->len, e->extent_offset, e->branch, e->sequence,
				e->access == FXFS_DAX_OPEN ? "writably" : "read-only",
				range->window_offset + (e->offset - start), err);
		return err;
	}
	bitmap_set(range->mapped, page, pages);
	if (e->access == FXFS_DAX_OPEN)
		fxfs_dax_set_writable(dax, range, page, pages);
	else
		fxfs_dax_clear_writable(range, page, pages);
	{
		unsigned long i;

		for (i = 0; i < pages; i++)
			fxfs_dax_trace(FXFS_DAX_TRACE_MAPPED, range->file->ino,
				       (start >> PAGE_SHIFT) + page + i,
				       fxfs_dax_page_offset(range, page + i), e->sequence,
				       e->extent_offset + ((u64)i << PAGE_SHIFT),
				       e->access == FXFS_DAX_OPEN);
	}
	return 0;
}

/* fxdax: maps piece `e` of `range`'s chunk in the range's own space, alone. */
static int fxfs_dax_map_own(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			    const struct fxfs_dax_extent *e)
{
	loff_t start = fxfs_dax_chunk_start(range);
	int err;

	err = fxfs_dax_device_map(dax, e->branch, e->sequence, e->extent_offset, e->len,
				  range->window_offset + (e->offset - start),
				  e->access == FXFS_DAX_OPEN, true);
	atomic64_inc(&dax->maps[FXFS_DAX_RANGE]);
	atomic64_add(e->len, &dax->map_bytes[FXFS_DAX_RANGE]);
	return err;
}

/*
 * Maps the pieces `extents` of the chunk `range` maps, at their places in it: open extents
 * writably, a range of its own in its own space, in a row with the rest of the chunk, which a fault
 * then maps at once (fxfs_dax_huge_fault, TASK-26.18.8). fxdax: those pieces all in one request, if
 * the device takes them so (fxfs_device_map_many, TASK-91.4), or one by one if that fails, to say
 * which did. In the gate if any is open, with the file's DAX semaphore held for writing.
 */
static int fxfs_dax_map_extents(struct fxfs_dax *dax, struct fxfs_dax_range *range,
				const struct fxfs_dax_extent *extents, ssize_t count)
{
	loff_t start = fxfs_dax_chunk_start(range);
	struct fxfs_device_map *maps = NULL;
	ssize_t i;
	int err, many = -ENOMEM;

	lockdep_assert_held_write(&range->file->sem);
	if (range->region)
		return fxfs_dax_map_slice(dax, range, extents, count);
	if (count > 1)
		maps = kmalloc_array(count, sizeof(*maps), GFP_NOFS);
	if (maps) {
		for (i = 0; i < count; i++) {
			const struct fxfs_dax_extent *e = &extents[i];

			maps[i] = (struct fxfs_device_map) {
				.branch = e->branch, .sequence = e->sequence,
				.offset = e->extent_offset, .len = e->len,
				.window_offset = range->window_offset + (e->offset - start),
				.writable = e->access == FXFS_DAX_OPEN,
				.ahead = true,
			};
		}
		many = fxfs_dax_device_map_many(dax, maps, count);
		kfree(maps);
		atomic64_add(count, &dax->maps[FXFS_DAX_RANGE]);
	}
	for (i = 0; i < count; i++) {
		const struct fxfs_dax_extent *e = &extents[i];

		if (!many) {
			atomic64_add(e->len, &dax->map_bytes[FXFS_DAX_RANGE]);
			err = 0;
		} else {
			/* If they failed, mapped again one by one, to say which, and why. */
			err = i && fxfs_dax_inject(&fxfs_dax_fail_fallbacks) ? -EIO :
			      fxfs_dax_map_own(dax, range, e);
		}
		err = fxfs_dax_mapped(dax, range, e, err);
		if (err)
			return err;
	}
	return 0;
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
		e->access = FXFS_DAX_SEALED;
		err = fxfs_dax_map_own(dax, range, e);
		/*
		 * An extent the device doesn't hold was never written: its blocks are zeros, a hole,
		 * as fxfs_dax_mapped has them; moved from where they were, which faults replace, and
		 * no longer writable, nor mapped, as the range had them (TASK-121).
		 */
		if (err == -ENOENT) {
			fxfs_dax_unmap_pages(range, page, pages);
			continue;
		}
		err = fxfs_dax_mapped(dax, range, e, err);
		if (err)
			return err;
	}
	return 0;
}

/*
 * fxdax: `range` didn't follow blocks Fxfs moved, as mapping them failed with `err`: it maps them
 * where they were, in an extent Fxfs frees, where the device puts zeros. Dropped, the chunk is mapped
 * from where Fxfs has them; until then nothing reads it. The kernel's mappings of it go, and the
 * iomaps of it say -ESTALE, whose callers drop it, each way in, whichever failed before: those
 * that gave up left it mapping the blocks, or never tried (TASK-150.13). Or -EXDEV of a slice
 * refusing blocks that moved, which is no failure, as often as writes move them: said to its
 * readers too, a fault that met two in a row, as -ESTALE, which it drops once, died of SIGBUS
 * (guest/sim, TASK-153.3). Returns what it says. With the file's DAX semaphore held for writing.
 */
static int fxfs_dax_fall_behind(struct inode *inode, struct fxfs_dax_range *range, int err)
{
	lockdep_assert_held_write(&range->file->sem);

	range->behind = err == -EXDEV ? err : -ESTALE;
	unmap_mapping_pages(inode->i_mapping, fxfs_dax_chunk_start(range) >> PAGE_SHIFT, range->pages,
			    false);
	return range->behind;
}

/* Whether any of the pieces `extents` is of an open extent. */
static bool fxfs_dax_any_open(const struct fxfs_dax_extent *extents, ssize_t count)
{
	ssize_t i;

	for (i = 0; i < count; i++) {
		if (extents[i].access == FXFS_DAX_OPEN)
			return true;
	}
	return false;
}

/*
 * fxdax: copying on write (doc-13). Whether `range` holds pages an UNMAP, or a MAP over them, would
 * lose: those of its file's dirty set, which no extent holds yet. Under its file's semaphore.
 */
static bool fxfs_dax_cow_busy(const struct fxfs_dax_range *range)
{
	return range->dirty && !bitmap_empty(range->dirty, range->pages);
}

/* fxdax: counts `private` more pages private, and `dirty` more dirty, of `file`'s, or fewer. */
static void fxfs_dax_cow_count(struct fxfs_dax *dax, struct fxfs_dax_file *file, long private,
			       long dirty)
{
	if (private) {
		atomic_long_add(private, &dax->private);
		atomic_long_add(private, &file->private_pages);
	}
	if (dirty) {
		atomic_long_add(dirty, &dax->dirty);
		atomic_long_add(dirty, &file->dirty_pages);
	}
	/* Room among the private pages, for what the budget holds up. */
	if (private < 0)
		wake_up_all(&dax->cow_wait);
}

/*
 * fxdax: has the budget's worker run a round (fxfs_dax_cow_work), which writes back and rebinds as
 * the private pages near private_max: and write back whatever is dirty, if `clean`, as a window
 * whose ranges are all dirty has nothing to reclaim. Not while it stops.
 */
static void fxfs_dax_cow_kick(struct fxfs_dax *dax, bool clean)
{
	if (clean)
		atomic_set(&dax->cow_clean, 1);
	if (!READ_ONCE(dax->cow_stopping))
		queue_work(dax->grow_wq, &dax->cow_work);
}

/*
 * fxdax: `range`'s `pages` pages from `page` lose what copying on write had of them, as an UNMAP,
 * or a truncate's MAP of zeros over them, ends their private pages: their dirty pages' room
 * released, dropped unwritten, as a truncate's, or an unlinked file's are; and, once the host has
 * freed them (`freed`), their private pages no longer counted. Not before the request ends: the
 * budget let another's store have the room, which the host, holding the pages still, refused past
 * private_max (guest/sim, seed 90). Nor if it failed, the host holding them for good: counted
 * still, lost, as the window's pages an UNMAP failed for are (fxfs_dax_give_unmapped). With its
 * file's semaphore held for writing.
 */
static void fxfs_dax_cow_forget(struct fxfs_dax *dax, struct fxfs_dax_range *range,
				unsigned long page, unsigned long pages, bool freed)
{
	struct fxfs_dax_file *file = range->file;
	unsigned long i, end = min(page + pages, range->pages);
	struct fxfs_dax_volume *volume;
	long private = 0, dirty = 0;

	if (!range->private)
		return;
	for (i = page; i < end; i++) {
		private += test_and_clear_bit(i, range->private);
		dirty += test_and_clear_bit(i, range->dirty);
		clear_bit(i, range->zeros);
	}
	fxfs_dax_cow_count(dax, file, freed ? -private : 0, -dirty);
	if (!freed)
		atomic_long_sub(private, &file->private_pages);
	/* A volume going (fxfs_dax_remove_volume) was synced: nothing of it is dirty. */
	volume = READ_ONCE(file->volume);
	if (dirty && !WARN_ON_ONCE(!volume))
		fxfs_release_dirty(volume->fs, file->ino, (u64)dirty << PAGE_SHIFT);
}

/*
 * fxdax: has `range` say what `map`, of its pages, mapped: an extent's page, or zeros. A private page
 * a MAP replaces is the VM's no more (rebinding): how many there were.
 */
static long fxfs_dax_cow_mapped(struct fxfs_dax *dax, struct fxfs_dax_range *range,
				const struct fxfs_device_map *map)
{
	unsigned long page = (map->window_offset - range->window_offset) >> PAGE_SHIFT;
	unsigned long end = page + (map->len >> PAGE_SHIFT), i;
	long rebound = 0;

	bitmap_set(range->mapped, page, end - page);
	for (i = page; i < end; i++) {
		if (map->zeros)
			set_bit(i, range->zeros);
		else
			clear_bit(i, range->zeros);
		rebound += test_and_clear_bit(i, range->private);
	}
	if (rebound)
		fxfs_dax_cow_count(dax, range->file, -rebound, 0);
	return rebound;
}

/*
 * fxdax: maps the `count` maps `maps`, of `range`'s pages, in as few requests as the device takes, or
 * one by one if that fails, to say which: a sealed extent the device doesn't hold (-ENOENT), never
 * written, as zeros, a hole, as Fxfs reads it. An extent open still (-EPERM: the window maps
 * none), Fxfs having taken it for sealed, as one minted before a seal and written after it is
 * (storage_device's OpenExtents), fails them, unless `skip_open`, when it is left as it is. Has the
 * range say what it mapped (fxfs_dax_cow_mapped): how many private pages that rebound, or the
 * first error. With its file's semaphore held for writing. fxdax: with fxfs.dax_fail_maps, a map
 * of a range's pages (not a rebind's, `skip_open`) of more than one piece maps the first, then
 * fails, as a device's MAP of the second may; with fxfs.dax_fail_fallbacks, the MAPs one by one
 * after a failed MAP_MANY fail past the first, as fxfs_dax_map_extents' do (TASK-164.2).
 */
static long fxfs_dax_cow_map_all(struct fxfs_dax *dax, struct fxfs_dax_range *range,
				 struct fxfs_device_map *maps, size_t count, bool skip_open)
{
	long rebound = 0;
	size_t i;
	int err;

	if (!count)
		return 0;
	if (count > 1 && !skip_open && fxfs_dax_inject(&fxfs_dax_fail_maps)) {
		rebound = fxfs_dax_cow_map_all(dax, range, maps, 1, skip_open);
		return rebound < 0 ? rebound : -EIO;
	}
	err = fxfs_dax_device_map_many(dax, maps, count);
	atomic64_add(count, &dax->maps[FXFS_DAX_RANGE]);
	for (i = 0; i < count; i++) {
		struct fxfs_device_map *map = &maps[i];

		if (err) {
			/* If they failed, mapped again one by one, to say which, and why. */
			int one = i && fxfs_dax_inject(&fxfs_dax_fail_fallbacks) ? -EIO :
				  map->zeros ?
				  fxfs_device_map_zeros(dax->dev, map->window_offset, map->len) :
				  fxfs_dax_device_map(dax, map->branch, map->sequence, map->offset,
						      map->len, map->window_offset, false, map->ahead);

			if (one == -ENOENT && !map->zeros) {
				map->zeros = true;
				one = fxfs_device_map_zeros(dax->dev, map->window_offset, map->len);
			}
			if (one == -EPERM)
				atomic64_inc(&dax->cow_open_maps);
			if (one == -EPERM && skip_open)
				continue;
			if (one) {
				pr_warn("fxfs: mapping %llu bytes at %llu of %s at %llu of the DAX window failed: %d\n",
					map->len, map->offset, map->zeros ? "zeros" : "an extent",
					map->window_offset, one);
				return one;
			}
		}
		atomic64_add(map->len, &dax->map_bytes[FXFS_DAX_RANGE]);
		rebound += fxfs_dax_cow_mapped(dax, range, map);
	}
	return rebound;
}

/* fxdax: whether any of the pieces `extents` is of an extent not known to be sealed. */
static bool fxfs_dax_any_unsealed(const struct fxfs_dax_extent *extents, ssize_t count)
{
	ssize_t i;

	for (i = 0; i < count; i++) {
		if (extents[i].access != FXFS_DAX_SEALED)
			return true;
	}
	return false;
}

/*
 * fxdax: copying on write, the pieces of file `ino`'s `len` bytes at `offset`, as fxfs_dax_extents,
 * once no seal was in progress as Fxfs looked: one says sealed from its beginning of what it has
 * yet to seal on the device (OpenExtents::seal, as fxfs-linux's Seal::begin_with has it), which
 * the host refuses to map (-EPERM). Looked up again after it, as long as seals keep coming:
 * mapping them, a range's map sealed and tried again, and a rebind left them private, in the
 * cow_ scenarios' schedules a mutant moved (cow_map_during_seal). A slice's region, likewise
 * (fxfs_dax_new_slice).
 */
static ssize_t fxfs_dax_cow_extents(struct fxfs_dax *dax, struct fxfs *fs, u64 ino, loff_t offset,
				    u64 len, struct fxfs_dax_extent *found, size_t capacity)
{
	ssize_t count;
	u64 seals;

	for (;;) {
		seals = fxfs_device_seals(dax->dev);
		count = fxfs_nofs(fxfs_dax_extents(fs, ino, offset, len, found, capacity));
		if (count < 0 || (!(seals & 1) && fxfs_device_seals(dax->dev) == seals))
			return count;
		fxfs_device_wait_unsealed(dax->dev);
	}
}

/*
 * fxdax: maps `range`'s pages `first` to `last`, exclusive, read-only, as copying on write has them
 * (doc-13): each from the sealed extent holding the file's bytes there, or zeros where it has none,
 * holes and pages past its end, so that every page of a range has a window page, which a store
 * makes private, and no write is new (IOMAP_F_NEW). Nothing maps an open extent: mapping the range
 * (`rebind` false), what is open is sealed first (fxfs_fsync), as a page-cache inode's writes read
 * in before their seal, or a growth's copy of the last block, may leave; rebinding private pages
 * (`rebind`), as their writeback left them, those whose extent is open stay private. How many
 * private pages it rebound, or a -errno. With the file's semaphore held for writing.
 */
static long fxfs_dax_cow_map(struct fxfs_dax *dax, struct fxfs *fs, struct fxfs_dax_range *range,
			     unsigned long first, unsigned long last, bool rebind)
{
	loff_t start = fxfs_dax_chunk_start(range);
	size_t capacity = last - first, n;
	struct fxfs_dax_extent *found;
	struct fxfs_device_map *maps;
	unsigned int tries;
	ssize_t count, i;
	long ret = 0;

	lockdep_assert_held_write(&range->file->sem);
	if (!capacity)
		return 0;
	found = fxfs_dax_alloc_extents(dax, capacity);
	maps = kmalloc_array(2 * capacity + 1, sizeof(*maps), GFP_NOFS);
	if (!found || !maps) {
		ret = -ENOMEM;
		goto out;
	}
	for (tries = 0;; tries++) {
		unsigned long page = first;

		count = fxfs_dax_cow_extents(dax, fs, range->file->ino,
					     start + ((loff_t)first << PAGE_SHIFT),
					     (u64)capacity << PAGE_SHIFT, found, capacity);
		if (count < 0) {
			ret = count;
			goto out;
		}
		if (rebind || !fxfs_dax_any_unsealed(found, count)) {
			n = 0;
			for (i = 0; i <= count; i++) {
				const struct fxfs_dax_extent *e = i < count ? &found[i] : NULL;
				unsigned long from = last, to;
				u64 at;

				if (e)
					from = clamp_t(unsigned long, (e->offset - start) >> PAGE_SHIFT,
						       page, last);
				/* The hole before it, or past the last: zeros. */
				if (from > page)
					maps[n++] = (struct fxfs_device_map) {
						.window_offset = fxfs_dax_page_offset(range, page),
						.len = (u64)(from - page) << PAGE_SHIFT,
						.zeros = true,
					};
				if (!e)
					break;
				to = clamp_t(unsigned long, (e->offset + e->len - start) >> PAGE_SHIFT,
					     from, last);
				at = start + ((u64)from << PAGE_SHIFT);
				if (to > from && e->access == FXFS_DAX_SEALED)
					maps[n++] = (struct fxfs_device_map) {
						.branch = e->branch, .sequence = e->sequence,
						.offset = e->extent_offset + (at - e->offset),
						.len = (u64)(to - from) << PAGE_SHIFT,
						.window_offset = fxfs_dax_page_offset(range, from),
						.ahead = !rebind,
					};
				page = max(page, to);
			}
			ret = fxfs_dax_cow_map_all(dax, range, maps, n, rebind);
			if (ret != -EPERM || rebind)
				break;
		}
		/*
		 * Each seal seals what was written before it began, which Fxfs committed before it
		 * reads the file's pieces: one is enough, unless writes go on meanwhile.
		 */
		if (tries == 3) {
			pr_warn("fxfs: ino %llu chunk %lu: extents open after %u seals\n",
				range->file->ino, range->chunk, tries);
			ret = -EIO;
			break;
		}
		atomic64_inc(&dax->cow_seals);
		ret = fxfs_nofs(fxfs_fsync(fs));
		if (ret)
			break;
	}
out:
	kfree(maps);
	kvfree(found);
	return ret;
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
 * fxdax: of the pieces `found`, those of `range`'s lagging pages, into `lagging`, one for each run,
 * of which there are no more than it has pages: how many.
 */
static ssize_t fxfs_dax_lagging_pieces(struct fxfs_dax_range *range,
				       const struct fxfs_dax_extent *found, ssize_t count,
				       struct fxfs_dax_extent *lagging)
{
	loff_t start = fxfs_dax_chunk_start(range);
	ssize_t i, nr = 0;

	for (i = 0; i < count; i++) {
		const struct fxfs_dax_extent *e = &found[i];
		unsigned long first = (e->offset - start) >> PAGE_SHIFT;
		unsigned long end = first + (e->len >> PAGE_SHIFT), page, next;

		end = min(end, range->pages);
		for (page = find_next_bit(range->lagging, end, first); page < end;
		     page = find_next_bit(range->lagging, end, next)) {
			u64 skipped = (u64)(page - first) << PAGE_SHIFT;

			next = find_next_zero_bit(range->lagging, end, page);
			lagging[nr] = *e;
			lagging[nr].offset += skipped;
			lagging[nr].extent_offset += skipped;
			lagging[nr++].len = (u64)(next - page) << PAGE_SHIFT;
		}
	}
	return nr;
}

/*
 * Maps the extents holding `inode`'s bytes from `start` to `end` of the chunk `range` is for: all of
 * them, or fxdax: if `lagging`, its lagging pages' alone, which then lag no more. For a fault
 * (`fault`), -EBUSY rather than wait for a seal (fxfs_dax_store_begin).
 */
static int fxfs_dax_map_found(struct fxfs_dax *dax, struct inode *inode,
			      struct fxfs_dax_range *range, loff_t start, loff_t end, bool lagging,
			      bool fault)
{
	size_t capacity = DIV_ROUND_UP(end - start, PAGE_SIZE);
	/* The runs of lagging pages after what Fxfs found: each a page at the least. */
	struct fxfs_dax_extent *found = fxfs_dax_alloc_extents(dax, lagging ? 2 * capacity : capacity);
	struct fxfs_dax_extent *pieces;
	ssize_t count, nr;
	long gate;
	u64 seals;
	int err;

	/* fxdax: not before, an offset from NULL (UBSan, guest/sim failures 408 at 1,000). */
	if (!found)
		return -ENOMEM;
	pieces = lagging ? found + capacity : found;
	for (;;) {
		/*
		 * Extents open as Rust finds them are still once the gate is entered, unless a seal
		 * was in progress then, or began since.
		 */
		seals = fxfs_device_seals(dax->dev);
		count = fxfs_nofs(fxfs_dax_extents(fxfs_vfs_fs(inode->i_sb), inode->i_ino, start, end - start,
						   found, capacity));
		fxfs_dax_delay();
		if (count < 0) {
			err = count;
			break;
		}
		nr = lagging ? fxfs_dax_lagging_pieces(range, found, count, pieces) : count;
		/* Sealed extents map read-only, whatever seals meanwhile, outside the gate. */
		if (!fxfs_dax_any_open(pieces, nr)) {
			err = fxfs_dax_map_extents(dax, range, pieces, nr);
			break;
		}
		gate = fxfs_dax_store_begin(dax, fault);
		if (gate < 0) {
			err = gate;
			break;
		}
		if (gate == seals) {
			err = fxfs_dax_map_extents(dax, range, pieces, nr);
			fxfs_device_store_end(dax->dev);
			break;
		}
		fxfs_device_store_end(dax->dev);
	}
	if (!err && lagging) {
		atomic64_inc(&dax->catch_ups);
		atomic64_add(bitmap_weight(range->lagging, range->pages), &dax->caught_up);
		/* Pages Fxfs has no blocks for are holes now, as a truncate or a punch left them. */
		for (count = 0; count < nr; count++)
			bitmap_clear(range->lagging, (pieces[count].offset - fxfs_dax_chunk_start(range)) >>
				     PAGE_SHIFT, pieces[count].len >> PAGE_SHIFT);
		fxfs_dax_unlag_holes(range);
	}
	kvfree(found);
	return err;
}

/*
 * Maps the extents holding the data of the chunk `range` is for, which is unmapped, as far as the
 * file goes, or the write it is for grows it to (`grows`): for a fault (`fault`), -EBUSY rather
 * than wait for a seal (fxfs_dax_store_begin). fxdax: a write grows the file as it ends, its
 * bytes written (fxfs_dax_iomap_end, TASK-133), so `grows` maps the extents it reserved past it.
 */
static int fxfs_dax_map_chunk(struct fxfs_dax *dax, struct inode *inode,
			      struct fxfs_dax_range *range, loff_t grows, bool fault)
{
	loff_t start = fxfs_dax_chunk_start(range);
	loff_t end = min_t(loff_t, start + ((loff_t)range->pages << PAGE_SHIFT),
			   round_up(max(i_size_read(inode), grows), PAGE_SIZE));
	long mapped;

	/* fxdax: copying on write, every page of it, zeros past the file's end (doc-13). */
	if (dax->cow) {
		mapped = fxfs_dax_cow_map(dax, fxfs_vfs_fs(inode->i_sb), range, 0, range->pages,
					  false);
		return mapped < 0 ? mapped : 0;
	}
	if (end <= start)
		return 0;
	return fxfs_dax_map_found(dax, inode, range, start, end, false, fault);
}

/*
 * fxdax: maps `range`'s lagging pages where Fxfs has them, in as few requests as it can: every one
 * of the chunk, for the first use of any. With the file's DAX semaphore held for writing.
 */
static int fxfs_dax_catch_up(struct fxfs_dax *dax, struct inode *inode,
			     struct fxfs_dax_range *range, bool fault)
{
	loff_t start = fxfs_dax_chunk_start(range);
	unsigned long first = find_first_bit(range->lagging, range->pages);
	unsigned long last = find_last_bit(range->lagging, range->pages);

	lockdep_assert_held_write(&range->file->sem);
	if (first >= range->pages)
		return 0;
	return fxfs_dax_map_found(dax, inode, range, start + ((loff_t)first << PAGE_SHIFT),
				  start + ((loff_t)(last + 1) << PAGE_SHIFT), true, fault);
}

/* fxdax: whether any of `range`'s `pages` pages from `page` lags, under its file's semaphore. */
static bool fxfs_dax_lags(const struct fxfs_dax_range *range, unsigned long page,
			  unsigned long pages)
{
	unsigned long end = min(page + pages, range->pages);

	return find_next_bit(range->lagging, end, page) < end;
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
 * fxdax: frees `pages` pages of the window at `window_offset`, which an UNMAP unmapped, unless it
 * failed (`err`): the host may map them still, which the range or region given them next would
 * read wherever it reads a page it hasn't mapped, as would a user's mapping the kernel didn't
 * replace. They are kept from use for good, lost to the window (TASK-44.22).
 */
static void fxfs_dax_give_unmapped(struct fxfs_dax *dax, u64 window_offset, unsigned long pages,
				   int err)
{
	if (!err) {
		fxfs_dax_give_window(dax, window_offset, pages);
		return;
	}
	pr_warn_ratelimited("fxfs: unmapping the DAX window at %llx failed: %d: its %lu pages kept from use\n",
			    window_offset, err, pages);
	atomic64_add((u64)pages << PAGE_SHIFT, &dax->lost_bytes);
}

/* Frees what `range` holds in memory, and it. */
static void fxfs_dax_free_range_memory(struct fxfs_dax_range *range)
{
	bitmap_free(range->mapped);
	bitmap_free(range->writable);
	bitmap_free(range->lagging);
	bitmap_free(range->private);
	bitmap_free(range->dirty);
	bitmap_free(range->zeros);
	kfree(range);
}

/*
 * A new range of `pages` pages at `window_offset`, unmapped and on no list: of its own, or a slice
 * of `region`, whose slice it takes. fxdax: with copy on write's state, if `cow`.
 */
static struct fxfs_dax_range *fxfs_dax_new_range(u64 window_offset, unsigned long pages,
						 struct fxfs_dax_region *region, bool cow)
{
	struct fxfs_dax_range *range;

	range = kzalloc(sizeof(*range), GFP_NOFS);
	if (!range)
		return ERR_PTR(-ENOMEM);
	range->mapped = bitmap_zalloc(pages, GFP_NOFS);
	range->writable = bitmap_zalloc(pages, GFP_NOFS);
	range->lagging = bitmap_zalloc(pages, GFP_NOFS);
	if (cow) {
		range->private = bitmap_zalloc(pages, GFP_NOFS);
		range->dirty = bitmap_zalloc(pages, GFP_NOFS);
		range->zeros = bitmap_zalloc(pages, GFP_NOFS);
	}
	if (!range->mapped || !range->writable || !range->lagging ||
	    (cow && (!range->private || !range->dirty || !range->zeros))) {
		fxfs_dax_free_range_memory(range);
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
	int err = 0;

	spin_lock(&dax->regions_lock);
	if (--region->refs) {
		spin_unlock(&dax->regions_lock);
		return;
	}
	hash_del(&region->node);
	list_del(&region->writable_list);
	spin_unlock(&dax->regions_lock);
	if (region->mapped)
		err = fxfs_dax_device_unmap(dax, region->window_offset,
					(u64)region->mapped << PAGE_SHIFT);
	fxfs_dax_give_unmapped(dax, region->window_offset, region->pages, err);
	kfree(region);
	atomic_long_dec(&dax->region_count);
	fxfs_dax_released(dax);
}

static void fxfs_dax_free_range_rcu(struct rcu_head *rcu)
{
	fxfs_dax_free_range_memory(container_of(rcu, struct fxfs_dax_range, rcu));
}

/*
 * Frees `range`, which nothing maps, and is in no file's ranges: its space in the window is free
 * again, unless its UNMAP failed with `unmap_err` (fxfs_dax_give_unmapped), or its slice is its
 * no more.
 */
static void fxfs_dax_free_range(struct fxfs_dax *dax, struct fxfs_dax_range *range, int unmap_err)
{
	spin_lock(&dax->lock);
	list_del(&range->list);
	spin_unlock(&dax->lock);
	if (range->region)
		fxfs_dax_put_region(dax, range->region);
	else
		fxfs_dax_give_unmapped(dax, range->window_offset, range->pages, unmap_err);
	call_rcu(&range->rcu, fxfs_dax_free_range_rcu);
	fxfs_dax_released(dax);
}

/*
 * Unmaps `range` and frees it, with its file's DAX semaphore held for writing, once nothing maps
 * its pages: its file's inode's mappings of them are gone (fxfs_dax_release), if it has one. A
 * slice's pages are its region's, which go with its last slice. fxdax: and no iomap or writeback
 * holds it (`users`), which reads the window at its place, given to another range next: WARNs if
 * one does, whichever way frees it (doc-13, GCE r544).
 */
static void fxfs_dax_unmap_range(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	int err = 0;

	lockdep_assert_held_write(&range->file->sem);
	WARN(atomic_read(&range->users),
	     "fxfs: ino %llu chunk %lu's range at %llx of the DAX window freed, %d using it\n",
	     range->file->ino, range->chunk, range->window_offset, atomic_read(&range->users));

	xa_erase(&range->file->ranges, range->chunk);
	if (!range->region && !bitmap_empty(range->mapped, range->pages))
		err = fxfs_dax_device_unmap(dax, range->window_offset,
					(u64)range->pages << PAGE_SHIFT);
	/* fxdax: copying on write, the UNMAP freed its private pages, unless it failed. */
	fxfs_dax_cow_forget(dax, range, 0, range->pages, !err);
	fxfs_dax_free_range(dax, range, err);
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
	if (file->inode || !xa_empty(&file->ranges) || !file->volume ||
	    xa_load(&file->volume->files, file->ino) != file)
		return false;
	xa_erase(&file->volume->files, file->ino);
	return true;
}

/*
 * Removes the inode's mappings of the chunk `range` maps, and the DAX entries for them, unless
 * a page is in use (pinned): with the inode's invalidate_lock held for writing. fxdax: and its
 * file's semaphore. Copying on write, unless the range holds dirty pages, which its UNMAP would
 * lose: their writeback (fxfs_dax_cow_write_back) takes the semaphore, and cleans the entries too.
 */
static bool fxfs_dax_release(struct inode *inode, struct fxfs_dax_range *range)
{
	struct address_space *mapping = inode->i_mapping;
	loff_t start = fxfs_dax_chunk_start(range);
	loff_t end = start + ((loff_t)range->pages << PAGE_SHIFT) - 1;
	struct writeback_control clean = {
		.sync_mode = WB_SYNC_ALL,
		.nr_to_write = LONG_MAX,
		.range_start = start,
		.range_end = end,
	};

	if (range->dirty && fxfs_dax_cow_busy(range))
		return false;
	if (dax_layout_busy_page_range(mapping, start, end))
		return false;
	/*
	 * Dirty entries go only once written back, which only cleans them. fxdax: copying on write,
	 * with no page dirty, what an entry says dirty is stale, cleaned here, as writepages would
	 * take the semaphore held: a writeback that met a fault holding its page's entry left it
	 * tagged (fxfs_dax_cow_protect), and none comes for a file with nothing dirty. Left so, the
	 * invalidation failed for ever, and a fault waiting for room reclaimed the range again and
	 * again (guest/sim, seed 3390 at 1,000 operations), as a drop of it waits.
	 */
	if (range->dirty ?
	    dax_writeback_mapping_range(mapping, fxfs_dax_of(inode)->dax_dev, &clean) :
	    filemap_fdatawrite_range(mapping, start, end))
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
		/* fxdax: copying on write, an evicted inode's were written back: kept if not. */
		if (range && !atomic_read(&range->users) && !fxfs_dax_cow_busy(range)) {
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
	long window_offset, seen, waited;
	unsigned int idle = 0;

	for (;;) {
		seen = atomic_long_read(&dax->released);
		window_offset = fxfs_dax_take_window(dax, pages, aligned);
		if (window_offset >= 0)
			return window_offset;
		if (fxfs_dax_reclaim(dax))
			continue;
		/*
		 * fxdax: copying on write, ranges holding dirty pages aren't reclaimed: written back,
		 * they are.
		 */
		if (dax->cow)
			fxfs_dax_cow_kick(dax, true);
		if (nowait)
			return -EAGAIN;
		/* Ranges in use, or whose inodes are locked, free up soon. */
		waited = fxfs_dax_wait_released(dax, seen, 10);
		if (waited < 0)
			return -EINTR;
		idle = waited ? 0 : idle + 1;
		/* fxdax: nothing let go of for so long isn't going to: space lost, or all pinned. */
		if (idle > FXFS_DAX_NO_ROOM_WAITS) {
			pr_warn_ratelimited("fxfs: no room in the window for %lu pages\n", pages);
			return -ENOSPC;
		}
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
	range = fxfs_dax_new_range(window_offset, pages, NULL, dax->cow);
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
 * Makes `region`, of an open extent, writable. In the gate, where only writable regions are mapped
 * further. Nothing has mapped it read-only: what maps a region read-only is a slice of a sealed
 * extent, which is never open again, and shared blocks make no slice (fxfs_dax_new_slice). A
 * read-only MAP meeting a writer here was how the host came to refuse stores, and stop the VM, and
 * a slice to refuse a write for ever (TASK-99, TASK-140).
 */
static void fxfs_dax_region_open(struct fxfs_dax *dax, struct fxfs_dax_region *region)
{
	fxfs_device_assert_in_gate(dax->dev);
	fxfs_dax_delay();
	spin_lock(&dax->regions_lock);
	if (!region->writable) {
		WARN_ONCE(READ_ONCE(region->mapped),
			  "fxfs: extent %llx:%llu open, its region mapped read-only\n",
			  region->branch, region->sequence);
		region->writable = true;
		list_add_tail(&region->writable_list, &dax->writable_regions);
	}
	spin_unlock(&dax->regions_lock);
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
 * Maps `region`'s extent as far as page `end` at least: read-only, all of it if it can, outside the
 * gate, whatever seals, a sealed extent's; or, in the gate, while the extent is open, writably, and
 * further, doubling what is mapped by up to fxfs_dax_region_step pages, from
 * FXFS_DAX_REGION_FIRST, so that the next blocks it hands out need no MAP. -EXDEV if the region's
 * space doesn't reach so far: sealed, it shrinks to what it mapped.
 *
 * A region is one or the other for as long as this looks. A piece found sealed was so after the
 * sealer forgot the region was writable, which nothing makes it again, as the extent is never open
 * again: no MAP of it outside the gate is writable, past a seal that gives the space past what is
 * mapped back (xfstests generic/013, TASK-44.21, TASK-150.11).
 */
static int fxfs_dax_map_to(struct fxfs_dax *dax, struct fxfs_dax_region *region, unsigned long end,
			   enum fxfs_dax_why why)
{
	unsigned long from, to;
	/* A seal clears it with no store in progress, so during no writable MAP. */
	bool writable;
	int err = 0;

	mutex_lock(&region->lock);
	writable = READ_ONCE(region->writable);
	from = region->mapped;
	if (end <= from)
		goto out;
	if (end > region->pages) {
		err = -EXDEV;
		goto out;
	}
	to = region->pages;
	if (writable)
		to = min(to, max(end, from + clamp(from, FXFS_DAX_REGION_FIRST,
						   READ_ONCE(fxfs_dax_region_step))));
	/* A read-only MAP may be a reader's, outside the gate; a writable one is in it. */
	if (writable)
		fxfs_device_assert_in_gate(dax->dev);
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
	} else if (err != -ENOENT && err != -ENOMEM)
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

	/* Outside the gate, a hint: a seal may shrink it meanwhile, which fxfs_dax_grow sees. */
	if (!READ_ONCE(region->writable) || mapped >= READ_ONCE(region->pages) ||
	    reached + clamp(mapped, FXFS_DAX_REGION_FIRST, READ_ONCE(fxfs_dax_region_step)) / 2 <=
	    mapped)
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
	int err = 0;

	fxfs_device_store_begin(dax->dev);
	/* In the gate, no seal makes it read-only, or shrinks it. */
	mapped = READ_ONCE(region->mapped);
	if (READ_ONCE(region->writable) && mapped < region->pages)
		err = fxfs_dax_map_to(dax, region, mapped + 1, FXFS_DAX_GROW);
	fxfs_device_store_end(dax->dev);
	atomic_set(&region->growing, 0);
	/* fxdax: not after a failure: a MAP that keeps failing went round here until a seal. */
	if (!err)
		fxfs_dax_grow_ahead(dax, region);
	fxfs_dax_put_region(dax, region);
	atomic_dec(&dax->growing);
}

/*
 * Maps `region`'s extent as far as page `end` at least (fxfs_dax_map_to), in the gate unless it is
 * sealed; and a writable region's further, in the background, ahead of the writes
 * (fxfs_dax_grow_ahead).
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
 * Makes `inode`'s chunk `chunk` a slice of its extent's region, if its data in the file's `size`
 * bytes is one piece of an extent the window maps whole, and returns it, kept, in its file's
 * ranges: writable if the extent is open. NULL if it isn't such a piece, its blocks are shared with
 * a clone while the extent is open, which a region of that extent can't map read-only for a writer
 * to find, or the window has no room for its region now; for a fault (`fault`), -EBUSY rather than
 * wait for a seal. With its file's DAX semaphore held for writing.
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

	if (end <= start || READ_ONCE(file->no_slices))
		return NULL;
	pages = (end - start) >> PAGE_SHIFT;
	for (;;) {
		seals = fxfs_device_seals(dax->dev);
		count = fxfs_nofs(fxfs_dax_extents(fxfs_vfs_fs(inode->i_sb), inode->i_ino, start, end - start, &e, 1));
		fxfs_dax_delay();
		/* Holes, or more than one piece. */
		if (count == 0 || count == -E2BIG)
			return NULL;
		if (count < 0)
			return ERR_PTR(count);
		if (e.offset != start || e.len != end - start || !e.extent_size ||
		    e.extent_size > FXFS_DAX_SIZE || !PAGE_ALIGNED(e.extent_offset) ||
		    e.access == FXFS_DAX_SHARED)
			return NULL;
		/*
		 * fxdax: copying on write, nothing maps an open extent: a range of its own maps the
		 * chunk, sealing it first (fxfs_dax_cow_map).
		 */
		if (dax->cow && e.access != FXFS_DAX_SEALED)
			return NULL;
		/*
		 * fxdax: nor what a seal in progress has yet to seal, which Fxfs says sealed from its
		 * beginning (fxfs_dax_cow_extents): looked up again after it, or, for a fault, -EBUSY,
		 * which waits for it without its DAX entry.
		 */
		if (dax->cow && ((seals & 1) || fxfs_device_seals(dax->dev) != seals)) {
			if (fault)
				return ERR_PTR(-EBUSY);
			fxfs_device_wait_unsealed(dax->dev);
			continue;
		}
		region = fxfs_dax_get_region(dax, &e, true);
		if (IS_ERR(region))
			return PTR_ERR(region) == -EAGAIN ? NULL : ERR_CAST(region);
		first = e.extent_offset >> PAGE_SHIFT;
		/* A sealed extent maps read-only, whatever seals meanwhile, outside the gate. */
		if (e.access != FXFS_DAX_OPEN)
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
				   region, dax->cow);
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
		fxfs_dax_free_range(dax, range, 0);
		goto out;
	}
	if (e.access == FXFS_DAX_OPEN)
		fxfs_dax_set_writable(dax, range, 0, pages);
	spin_lock(&dax->lock);
	list_add_tail(&range->list, &dax->used);
	spin_unlock(&dax->lock);
	if (e.access == FXFS_DAX_OPEN)
		fxfs_device_store_end(dax->dev);
	return range;
out:
	if (e.access == FXFS_DAX_OPEN)
		fxfs_device_store_end(dax->dev);
	if (region)
		fxfs_dax_put_region(dax, region);
	/*
	 * Its region can't reach it, or its extent was never written: a range of its own maps it,
	 * or the holes it is.
	 */
	return err == -EXDEV || err == -ENOENT || (dax->cow && err == -EPERM) ? NULL : ERR_PTR(err);
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
 * own, its extents mapped as far as the write it is for grows the file to (`grows`,
 * fxfs_dax_map_chunk). For a fault (`fault`), -EBUSY rather than wait for a seal
 * (fxfs_dax_store_begin), and fxdax: -EDEADLK rather than for the file's semaphore
 * (fxfs_dax_down_read). fxdax: no slice unless `slice`: a copy-on-write store's chunk is a range of
 * its own (doc-13).
 */
static struct fxfs_dax_range *fxfs_dax_get_range_timed(struct inode *inode, pgoff_t chunk,
						       loff_t size, loff_t grows, bool fault,
						       bool timed, bool slice)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range, *new;
	unsigned long pages;
	u64 began = ktime_get_ns();
	int err;

	err = fxfs_dax_down_read(file, fault);
	if (timed)
		atomic64_add(ktime_get_ns() - began, &dax->write_range_wait_ns);
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
	else if (slice)
		range = fxfs_dax_new_slice(dax, inode, chunk, max3(i_size_read(inode), size,
								   READ_ONCE(file->growing)), fault);
	up_write(&file->sem);
	if (IS_ERR(range))
		return range;
	if (range)
		goto used;

	pages = fxfs_dax_range_capacity(chunk, max3(i_size_read(inode), size,
						    READ_ONCE(file->growing)));
	new = fxfs_dax_alloc_range(dax, pages, fault);
	if (IS_ERR(new))
		return new;
	err = fxfs_dax_down_write(file, fault);
	if (err) {
		fxfs_dax_free_range(dax, new, 0);
		return ERR_PTR(err);
	}
	/* Another iomap may have mapped it meanwhile. */
	range = xa_load(&file->ranges, chunk);
	if (range) {
		atomic_inc(&range->users);
		up_write(&file->sem);
		fxfs_dax_free_range(dax, new, 0);
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
		err = fxfs_dax_map_chunk(dax, inode, new, grows, fault);
		/* Its one user, this, which no one else saw, the semaphore held throughout. */
		if (err) {
			atomic_dec(&new->users);
			fxfs_dax_unmap_range(dax, new);
		}
	} else {
		fxfs_dax_free_range(dax, new, 0);
	}
	up_write(&file->sem);
	if (err)
		return ERR_PTR(err);
	range = new;
used:
	fxfs_dax_use_range(dax, range);
	return range;
}

static struct fxfs_dax_range *fxfs_dax_get_range(struct inode *inode, pgoff_t chunk, loff_t size,
						 loff_t grows, bool fault)
{
	return fxfs_dax_get_range_timed(inode, chunk, size, grows, fault, false, true);
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
		bool busy = false, dirty = false;

		down_write(&file->sem);
		xa_for_each_range(&file->ranges, chunk, range, first, last) {
			if (size >= 0 && range->pages >= fxfs_dax_range_pages(chunk, size))
				continue;
			/*
			 * fxdax: copying on write, its dirty pages are written back first: the UNMAP
			 * frees their private pages, and the range maps again what Fxfs then has.
			 */
			dirty = fxfs_dax_cow_busy(range);
			if (dirty || atomic_read(&range->users) || !fxfs_dax_release(inode, range)) {
				busy = true;
				break;
			}
			fxfs_dax_unmap_range(dax, range);
		}
		up_write(&file->sem);
		if (!busy)
			return 0;
		if (dirty) {
			err = fxfs_dax_cow_write_back(inode);
			if (err)
				return err;
			continue;
		}
		/* fxdax: a page pinned, which a fault in the kernel doesn't wait for. */
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
	/*
	 * fxdax: copying on write, Fxfs takes a file's size as its writeback writes its pages, as the
	 * page cache's: the dirty pages are written back, which a write(2)'s leave the mapping
	 * untagged for, so that writepages isn't called for them (doc-13).
	 */
	if (fxfs_dax_of(inode)->cow)
		return fxfs_dax_cow_write_back(inode);
	filemap_invalidate_lock_shared(inode->i_mapping);
	down_write(&file->sem);
	err = fxfs_dax_write_size_locked(inode);
	up_write(&file->sem);
	filemap_invalidate_unlock_shared(inode->i_mapping);
	return err;
}

/*
 * Drops `inode`'s range of the chunk at `pos`, a slice a write can't use (-EXDEV), to be mapped
 * again where its data now is. fxdax: unless `pinned`, -EBUSY rather than wait for pages pinned
 * (TASK-79.49).
 */
static int fxfs_dax_drop_slice(struct inode *inode, loff_t pos, bool pinned)
{
	int err;

	filemap_invalidate_lock(inode->i_mapping);
	err = fxfs_dax_drop_pinned(inode, pos >> FXFS_DAX_SHIFT, pos >> FXFS_DAX_SHIFT, -1, pinned);
	filemap_invalidate_unlock(inode->i_mapping);
	return err;
}

/*
 * fxdax: drops the slice a fault can't use: 0 to try again, or what it returns, which is `ret` if
 * the slice stays. A fault from user mode holds no pins, so it waits for those others hold, as a
 * truncate does, the mapping lock held, and a signal sends it back to fault again. One from the
 * kernel may hold them itself, as get_user_pages does while it faults, and fails rather than wait
 * for itself (TASK-79.49).
 */
static vm_fault_t fxfs_dax_fault_drop_slice(struct vm_fault *vmf, vm_fault_t ret)
{
	bool user = vmf->flags & FAULT_FLAG_USER;
	int err = fxfs_dax_drop_slice(file_inode(vmf->vma->vm_file),
				      (loff_t)vmf->pgoff << PAGE_SHIFT, user);

	if (!err)
		return 0;
	return user && (err == -EINTR || err == -ERESTARTSYS) ? VM_FAULT_NOPAGE : ret;
}

/*
 * fxdax: makes room for `len` bytes at `pos`, as fxfs_dax_map_write does, or, for a write(2) whose
 * move commits after its bytes (`pending`), as fxfs_dax_begin_write does.
 */
static ssize_t fxfs_dax_room_for_write(struct inode *inode, loff_t pos, loff_t len, loff_t size,
				       bool keep, const void *held, struct fxfs_dax_extent *extents,
				       size_t capacity, struct fxfs_dax_write **pending)
{
	struct fxfs *fs = fxfs_vfs_fs(inode->i_sb);

	atomic64_inc(&fxfs_dax_of(inode)->map_writes);
	if (pending)
		return fxfs_nofs(fxfs_dax_begin_write(fs, inode->i_ino, pos, len, size, extents,
						      capacity, pending));
	return fxfs_nofs(fxfs_dax_map_write(fs, inode->i_ino, pos, len, size, keep, held, extents,
					    capacity));
}

/*
 * Makes room through `reserved`, recording only `size`, with an exact-write fallback. fxdax: the
 * blocks hold `held`, if not NULL, which is `reserved - pos` bytes (fxfs_dax_map_write); or the
 * move is a write(2)'s, committed after its bytes, if `pending` isn't NULL (fxfs_dax_begin_write).
 */
static ssize_t fxfs_dax_reserve_write(struct inode *inode, loff_t pos, loff_t end,
				    loff_t reserved, loff_t size, bool keep, const void *held,
				    struct fxfs_dax_extent *extents, size_t capacity,
				    struct fxfs_dax_write **pending)
{
	ssize_t count;

	count = fxfs_dax_room_for_write(inode, pos, reserved - pos, size, keep, held, extents,
					capacity, pending);
	/* Speculation must not refuse a write that its own bytes have room for. */
	if (!held && reserved > end &&
	    (count == -ENOSPC || count == -EDQUOT || count == -ENOMEM))
		count = fxfs_dax_room_for_write(inode, pos, end - pos, size, keep, NULL, extents,
						capacity, pending);
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

/*
 * fxdax: whether a write of the bytes from `pos` to `end` overwrites whole pages of the file's data,
 * which a move committed before its bytes would lose: moving a whole page, Fxfs keeps nothing of
 * what it held, for the write to fill, and a copy from the user's memory can stop short, where the
 * memory ends, or be of those very pages, or the write fail before it copies (TASK-150.8, 79.106,
 * 119.13). Such a write's move commits after its bytes (fxfs_dax_begin_write). A fault keeps what
 * it moves, zeroing is meant to lose it, and pages past the file's end hold nothing.
 */
static bool fxfs_dax_overwrites(struct inode *inode, unsigned int flags, loff_t pos, loff_t end)
{
	loff_t last = min(round_down(end, PAGE_SIZE), round_up(i_size_read(inode), PAGE_SIZE));

	return !(flags & (IOMAP_FAULT | IOMAP_ZERO)) && round_up(pos, PAGE_SIZE) < last;
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
					     size, fault, NULL, extents, capacity, NULL);
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
 * Maps a read of up to `length` bytes at `pos`: a run of mapped pages, or of holes.
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
		range = fxfs_dax_get_range(inode, pos >> FXFS_DAX_SHIFT, 0, 0, fault);
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
	if (range->behind) {
		err = range->behind;
		up_read(&file->sem);
		fxfs_dax_put_range(dax, range);
		return err;
	}
	start = fxfs_dax_chunk_start(range);
	page = (pos - start) >> PAGE_SHIFT;
	/* A range covers the file's chunk as its size grows (fxfs_dax_fit). */
	if (WARN_ON_ONCE(page >= range->pages)) {
		up_read(&file->sem);
		fxfs_dax_put_range(dax, range);
		return -EIO;
	}
	/* fxdax: what it reads, in the window at last, with the rest that lagged. */
	if (fxfs_dax_lags(range, page, DIV_ROUND_UP(offset_in_page(pos) + length, PAGE_SIZE))) {
		up_read(&file->sem);
		err = fxfs_dax_down_write(file, fault);
		if (!err) {
			err = range->behind ?: fxfs_dax_catch_up(dax, inode, range, fault);
			up_write(&file->sem);
		}
		if (err) {
			fxfs_dax_put_range(dax, range);
			return err;
		}
		err = fxfs_dax_down_read(file, fault);
		if (err) {
			fxfs_dax_put_range(dax, range);
			return err;
		}
	}
	mapped = test_bit(page, range->mapped);
	end_page = mapped ? find_next_zero_bit(range->mapped, range->pages, page) :
			    find_next_bit(range->mapped, range->pages, page);
	if (mapped) {
		at = fxfs_dax_page_offset(range, page);
		end_page = min_t(unsigned long, end_page,
				 page + DIV_ROUND_UP(offset_in_page(pos) + length, PAGE_SIZE));
	}
	length = min_t(loff_t, length, start + ((loff_t)end_page << PAGE_SHIFT) - pos);
	if (!mapped) {
		up_read(&file->sem);
		fxfs_dax_put_range(dax, range);
		fxfs_dax_hole(iomap, length);
		return 0;
	}
	/* fxdax: a page's place within the window, or its fault dies of SIGBUS (TASK-121). */
	WARN(at + length > dax->window_len,
	     "fxfs: ino %lu page %lu read at %llx, %llx bytes, past the window's %llx\n",
	     inode->i_ino, page, at, length, dax->window_len);
	iomap->type = IOMAP_MAPPED;
	iomap->addr = at + offset_in_page(pos);
	iomap->length = length;
	iomap->private = range;
	if (flags & IOMAP_FAULT) {
		fxfs_dax_trace(FXFS_DAX_TRACE_READ, inode->i_ino, (start >> PAGE_SHIFT) + page, at, 0,
			       length >> PAGE_SHIFT, 0);
		if (length >= PMD_SIZE)
			atomic64_inc(&dax->huge_maps);
		/*
		 * fxdax: a slice's page may be in other files' DAX entries too, or this one's at
		 * another offset, as blocks of a staging extent or a reflink's are: said so, as XFS
		 * says of its reflinks' reads, or the kernel takes the page for one no entry has
		 * (dax_associate_entry's WARN, xfstests generic/075, TASK-140).
		 */
		if (range->region)
			iomap->flags |= IOMAP_F_SHARED;
		/* fxdax: until its iomap_end, as moves wait for it (`faulting`). */
		atomic_inc(&range->faulting);
	}
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

/* fxdax: the window of the filesystem mounted last, for fxfs.dax_stray; it goes as the mount does. */
static struct fxfs_dax *fxfs_dax_stray_dax;

/*
 * fxdax: what a guest that keeps to no protocol does, for tests (TASK-77): `load N` or `store N`,
 * written to fxfs.dax_stray, reads or stores a word at byte N of the window, whatever maps it.
 */
static int fxfs_dax_stray_set(const char *value, const struct kernel_param *kp)
{
	struct fxfs_dax *dax = READ_ONCE(fxfs_dax_stray_dax);
	char op[6];
	u64 offset;
	u64 *at;
	int id, err = -EINVAL;

	if (!dax || sscanf(value, "%5s %llu", op, &offset) != 2 || offset >= dax->window_len)
		return -EINVAL;
	id = dax_read_lock();
	at = fxfs_dax_window_page(dax, offset);
	if (at) {
		at = (void *)at + offset_in_page(offset);
		if (!strcmp(op, "load")) {
			pr_info("fxfs: a stray load at %llu of the window: %llx\n", offset, READ_ONCE(*at));
			err = 0;
		} else if (!strcmp(op, "store")) {
			WRITE_ONCE(*at, 0x5a5a5a5a5a5a5a5aULL);
			err = 0;
		}
	}
	dax_read_unlock(id);
	return err;
}

static const struct kernel_param_ops fxfs_dax_stray_ops = { .set = fxfs_dax_stray_set };
module_param_cb(dax_stray, &fxfs_dax_stray_ops, NULL, 0200);
MODULE_PARM_DESC(dax_stray, "`load N` or `store N`: access byte N of the window, whatever maps it");

/*
 * fxdax: with fxfs.dax_check_moves and the shadow, logs where `seen`, the window's page at
 * `window_offset` as copied, differs from the extent's page the shadow has the window map there,
 * as the device has it; and where Fxfs has file `inode`'s page at `pos` elsewhere, and what the
 * device has there: what a write that moves the page reads around what it writes (TASK-121).
 * Returns where the window maps it, as fxfs_dax_read_shadowed does. fxdax: a page the shadow
 * doesn't know of it can't place, and finds nothing wrong with: it said Fxfs had it elsewhere than
 * the window, which it said mapped extent -1, and failed a test (TASK-119.21, GCE r239).
 */
static s64 fxfs_dax_check_device(struct fxfs_dax *dax, struct inode *inode, loff_t pos,
				 u64 window_offset, const u8 *seen)
{
	u8 *buf = READ_ONCE(fxfs_dax_shadowed) ? kmalloc(PAGE_SIZE, GFP_NOFS) : NULL;
	struct fxfs_dax_extent e;
	size_t first, differ;
	ssize_t count;
	s64 where;
	int err;

	if (!buf)
		return -ENOMEM;
	where = fxfs_dax_read_shadowed(dax, window_offset >> PAGE_SHIFT, buf);
	differ = where >= 0 ? fxfs_dax_differ(seen, buf, 0, 0, &first, PAGE_SIZE) : 0;
	if (differ) {
		pr_err("fxfs: window %llx, mapping extent %lld page %lld, held %zu bytes the device's doesn't before a move, the first at %zu: %#x, the device's %#x\n",
		       window_offset, where >> 20, where & ((1 << 20) - 1), differ, first, seen[first],
		       buf[first]);
		fxfs_dax_diagnosed();
	}
	count = fxfs_nofs(fxfs_dax_extents(fxfs_vfs_fs(inode->i_sb), inode->i_ino, pos, PAGE_SIZE,
					   &e, 1));
	if (count != 1) {
		pr_err("fxfs: ino %lu at %lld, mapped at window %llx: %zd extents in Fxfs\n",
		       inode->i_ino, pos, window_offset, count);
	} else if (where >= 0 && (e.sequence != where >> 20 ||
				  e.extent_offset >> PAGE_SHIFT != (where & ((1 << 20) - 1)))) {
		err = fxfs_dax_read_device_page(dax, e.branch, e.sequence, e.extent_offset, buf);
		differ = err ? 0 : fxfs_dax_differ(seen, buf, 0, 0, &first, PAGE_SIZE);
		pr_err("fxfs: ino %lu at %lld: Fxfs has it in extent %llu page %llu, %s, the window at %llx maps extent %lld page %lld: %d, %zu bytes differ, the first at %zu: %#x, the device's %#x\n",
		       inode->i_ino, pos, e.sequence, e.extent_offset >> PAGE_SHIFT,
		       fxfs_dax_accesses[e.access], window_offset, where >> 20,
		       where & ((1 << 20) - 1), err, differ, differ ? first : 0,
		       differ ? seen[first] : 0, differ ? buf[first] : 0);
		/* The same bytes, where the range hasn't followed them: wrong only if they differ. */
		if (differ)
			fxfs_dax_diagnosed();
		/* fxdax: how it fell behind, what the trace, if on, has of the page (TASK-133). */
		if (READ_ONCE(fxfs_dax_trace_events)) {
			char of[48];

			snprintf(of, sizeof(of), "%lu %llu", inode->i_ino,
				 (unsigned long long)pos >> PAGE_SHIFT);
			fxfs_dax_trace_dump(of, NULL);
		}
	}
	kfree(buf);
	return where;
}

/*
 * fxdax: with fxfs.dax_check_moves, WARNs unless `range`'s page `page`, where it moved, holds what
 * it held at `before_at` (`before`), but for the `len` bytes from `skip` a write writes (TASK-121).
 * In the gate, which keeps both still.
 */
static void fxfs_dax_check_moved(struct fxfs_dax *dax, struct inode *inode,
				 struct fxfs_dax_range *range, unsigned long page, const u8 *before,
				 u64 before_at, s64 before_where, size_t skip, size_t len)
{
	u64 at = fxfs_dax_page_offset(range, page);
	/* What the page held past the file's end, a mapping's store, isn't the file's to keep. */
	size_t in_file = clamp_t(loff_t, i_size_read(inode) - fxfs_dax_chunk_start(range) -
				 ((loff_t)page << PAGE_SHIFT), 0, PAGE_SIZE);
	int id = dax_read_lock();
	const u8 *now = fxfs_dax_window_page(dax, at), *then;
	size_t first, differ = now ? fxfs_dax_differ(now, before, skip, len, &first, in_file) : 0;
	u8 moved = differ ? now[first] : 0, left = 0;
	s64 is;
	u8 *buf;
	int err;

	/* Where it was, as the window has it now. */
	then = differ ? fxfs_dax_window_page(dax, before_at) : NULL;
	if (then)
		left = then[first];
	dax_read_unlock(id);
	if (!differ)
		return;
	pr_err("fxfs: ino %lu page %lu moved from %llx to %llx holds %zu other bytes, the first at %zu: %#x, not %#x\n",
	       inode->i_ino, (unsigned long)(fxfs_dax_chunk_start(range) >> PAGE_SHIFT) + page,
	       before_at, at, differ, first, moved, before[first]);
	fxfs_dax_diagnosed();
	/* fxdax: what the trace, if on, has of the page. */
	if (READ_ONCE(fxfs_dax_trace_events)) {
		char of[48];

		snprintf(of, sizeof(of), "%lu %lu", inode->i_ino,
			 (unsigned long)(fxfs_dax_chunk_start(range) >> PAGE_SHIFT) + page);
		fxfs_dax_trace_dump(of, NULL);
	}
	/*
	 * With the shadow, the extents' pages the window maps at both, as the device has them: the
	 * window's view of where the page was against what the device holds there, which a write
	 * reads around what it writes (TASK-121).
	 */
	buf = READ_ONCE(fxfs_dax_shadowed) ? kmalloc(PAGE_SIZE, GFP_NOFS) : NULL;
	if (!buf)
		return;
	err = before_where < 0 ? before_where :
	      fxfs_dax_read_device_page(dax, READ_ONCE(dax->shadow_branch), before_where >> 20,
					(before_where & ((1 << 20) - 1)) << PAGE_SHIFT, buf);
	if (!err)
		pr_err("fxfs:   from %llx, mapping extent %lld page %lld, the device has %#x there, the window %#x now\n",
		       before_at, before_where >> 20, before_where & ((1 << 20) - 1), buf[first],
		       left);
	else
		pr_err("fxfs:   from %llx: %d\n", before_at, err);
	is = fxfs_dax_read_shadowed(dax, at >> PAGE_SHIFT, buf);
	if (is >= 0)
		pr_err("fxfs:   to %llx, mapping extent %lld page %lld, the device has %#x there\n",
		       at, is >> 20, is & ((1 << 20) - 1), buf[first]);
	else
		pr_err("fxfs:   to %llx: %lld\n", at, is);
	kfree(buf);
}

/* fxdax: drops a write(2)'s move, `pending`, uncommitted, which leaves the file as it was. */
static void fxfs_dax_drop_pending(struct fxfs_dax *dax, struct fxfs_dax_write *pending)
{
	atomic64_inc(&dax->writes_dropped);
	fxfs_nofs(fxfs_dax_end_write(pending, false));
}

/*
 * fxdax: whether a write(2) (`flags`) of `range`'s `pages` pages from `first`, whose blocks went to
 * the one piece `e`, may go through its extent's region: a staging extent's, open, as a slice's is,
 * the pages none of them writable where they are, so that they all moved. The range may be a
 * slice, of the region they moved out of. No fault past its iomap uses it, as the write waited for
 * them: one about to map a page mapped where its blocks were, for good (fxfs_dax_iomap_write).
 */
static bool fxfs_dax_goes_through(struct fxfs_dax_range *range,
				  const struct fxfs_dax_extent *e, unsigned long first,
				  unsigned long pages, unsigned int flags)
{
	loff_t start = fxfs_dax_chunk_start(range) + ((loff_t)first << PAGE_SHIFT);

	return READ_ONCE(fxfs_dax_writes_through) && !(flags & (IOMAP_FAULT | IOMAP_ZERO)) &&
	       e->access == FXFS_DAX_OPEN && e->extent_size && e->extent_size <= FXFS_DAX_SIZE &&
	       PAGE_ALIGNED(e->extent_offset) && (e->extent_offset || e->len < e->extent_size) &&
	       e->offset <= start && e->offset + e->len >= start + ((loff_t)pages << PAGE_SHIFT) &&
	       first + pages <= range->pages &&
	       find_next_bit(range->writable, first + pages, first) == first + pages;
}

/*
 * fxdax: has the region of the piece `e` writable as far as the bytes from `pos` to `end`, kept for
 * the iomap (`file`'s `through`), and says where in the window `pos` is: -ENOSPC or -EAGAIN if the
 * window has no room for it now, -EXDEV if its space doesn't reach them, when the range's own space
 * maps the piece. The next writes keep it too, as the spare: else each mapped it anew. In the gate.
 */
static int fxfs_dax_open_through(struct fxfs_dax *dax, struct fxfs_dax_file *file,
				 const struct fxfs_dax_extent *e, loff_t pos, loff_t end, u64 *at)
{
	u64 offset = e->extent_offset + (pos - e->offset);
	struct fxfs_dax_region *region, *old = NULL;
	int err;

	/* Not reclaiming other ranges for it, in the gate: the range's own space maps it then. */
	region = fxfs_dax_get_region(dax, e, false);
	if (IS_ERR(region))
		return PTR_ERR(region);
	fxfs_dax_region_open(dax, region);
	err = fxfs_dax_reach(dax, region, DIV_ROUND_UP(offset + (end - pos), PAGE_SIZE));
	if (err) {
		fxfs_dax_put_region(dax, region);
		return err;
	}
	spin_lock(&dax->regions_lock);
	if (READ_ONCE(dax->restaging) && dax->spare != region) {
		old = dax->spare;
		dax->spare = region;
		region->refs++;
	}
	spin_unlock(&dax->regions_lock);
	if (old)
		fxfs_dax_put_region(dax, old);
	file->through = region;
	*at = region->window_offset + offset;
	return 0;
}

/*
 * Maps a write of up to `length` bytes at `pos` writably, making room for it in open extents
 * unless its pages are writable already, and returns in the gate, which fxfs_dax_iomap_end leaves.
 * A zeroing write leaves holes be. A write past the file's end grows it, unless it is a fault's.
 * fxdax: a write(2)'s move of its blocks commits there too, after its bytes, out of the gate, the
 * file's semaphore held from here until then (`pending`, TASK-153.5).
 */
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
	/*
	 * Whether pages it maps were holes, which mappings of the file may map zeros for: and only
	 * those, in a range that says so. A chunk with no range yet has no entries to replace but what a
	 * fault mapped since, data, a PMD's maybe, which the kernel's entries of new pages then lost
	 * from under its mapping (dax_folio_put's WARN).
	 */
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
	s64 before_where = -ENOENT;
	bool fault = flags & IOMAP_FAULT;
	/*
	 * fxdax: whether a move of this iomap's committed, after which the window's page maps where the
	 * blocks were, which a flush may have freed: nothing is taken from it then. A fault whose page
	 * moved with nothing held, its allocation failed, and was sealed from the gate, held the zeros
	 * of the freed block on the next pass, and made them the page's (guest/sim, failures 784 at
	 * 1,000).
	 */
	bool moved_once = false;
	/* A write(2)'s, of whose time write_ns counts. */
	bool timed = !(flags & (IOMAP_FAULT | IOMAP_ZERO));
	ssize_t count, moved = 0;
	long gate;
	u64 seals, began;
	/* fxdax: where in the window its bytes go, if through a region (fxfs_dax_open_through). */
	u64 through = 0;
	/*
	 * fxdax: a write(2)'s move, its transaction not committed until fxfs_dax_iomap_end, once the
	 * bytes are in: until then the file has its old blocks, which the window maps where it doesn't
	 * map the new ones, and nothing is lost if the write fails, as it was when the move committed
	 * first (TASK-153.5). A fault's bytes come with the user's store, as late as it likes: its
	 * move commits first, keeping what the page holds.
	 */
	struct fxfs_dax_write *pending = NULL;
	int err = 0;

	/* Faults and zeroing don't grow the file. */
	size = flags & (IOMAP_FAULT | IOMAP_ZERO) ? 0 :
	       min_t(loff_t, pos + length, round_up(pos + 1, FXFS_DAX_SIZE));
	reserved = size ? fxfs_dax_append_end(inode, pos, size) : 0;
	end = max(reserved, min_t(loff_t, pos + length, round_up(pos + 1, FXFS_DAX_SIZE)));
	capacity = DIV_ROUND_UP(offset_in_page(pos) + end - pos, PAGE_SIZE);
	/*
	 * A chunk with no range yet: room for the write first. Zeroing leaves holes be. fxdax: unless
	 * the write overwrites the file's data, whose move the loop below makes, to commit after the
	 * bytes: moved first, a failure after the move lost the file's data (TASK-79.106, 119.13).
	 */
	if (!(flags & IOMAP_ZERO) && !xa_load(&file->ranges, pos >> FXFS_DAX_SHIFT) &&
	    !fxfs_dax_overwrites(inode, flags, pos, end)) {
		extents = fxfs_dax_alloc_extents(dax, capacity);
		if (!extents)
			return -ENOMEM;
		began = ktime_get_ns();
		err = fxfs_dax_prepare_write(inode, pos,
					     min_t(loff_t, pos + length, round_up(pos + 1, FXFS_DAX_SIZE)) - pos,
					     size, reserved, fault, extents, capacity);
		if (timed)
			atomic64_add(ktime_get_ns() - began, &dax->write_prepare_ns);
		if (err) {
			kvfree(extents);
			return err;
		}
	}
	began = ktime_get_ns();
	range = fxfs_dax_get_range_timed(inode, pos >> FXFS_DAX_SHIFT, max(size, reserved), size,
					 fault, timed, true);
	/* Nor may reserving window space refuse a write whose own range fits. */
	if (IS_ERR(range) && reserved > size &&
	    (PTR_ERR(range) == -ENOMEM || PTR_ERR(range) == -ENOSPC))
		range = fxfs_dax_get_range_timed(inode, pos >> FXFS_DAX_SHIFT, size, size, fault, timed,
						 true);
	if (timed)
		atomic64_add(ktime_get_ns() - began, &dax->write_range_ns);
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
		if (timed) {
			atomic64_inc(&dax->write_waits);
			atomic64_add(ktime_get_ns() - began, &dax->write_wait_ns);
		}
	}
	if (range->behind) {
		err = range->behind;
		goto out;
	}
	/* fxdax: a page lagging is in the window first, as the next steps have it. */
	if (fxfs_dax_lags(range, first, pages)) {
		err = fxfs_dax_catch_up(dax, inode, range, fault);
		if (err)
			goto out;
	}
	for (;;) {
		began = ktime_get_ns();
		gate = fxfs_dax_store_begin(dax, fault);
		if (timed)
			atomic64_add(ktime_get_ns() - began, &dax->write_gate_ns);
		if (gate < 0) {
			err = gate;
			goto out;
		}
		/* Writable pages take the write as they are. */
		if (find_next_zero_bit(range->writable, first + pages, first) == first + pages)
			break;
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
		    !before && !moved_once) {
			int id = dax_read_lock();
			void *page;

			before_at = fxfs_dax_page_offset(range, first);
			page = fxfs_dax_window_page(dax, before_at);
			before = page ? kmalloc(PAGE_SIZE, GFP_NOFS) : NULL;
			if (before)
				memcpy(before, page, PAGE_SIZE);
			dax_read_unlock(id);
			if (before)
				before_where = fxfs_dax_check_device(dax, inode,
								     start + ((loff_t)first << PAGE_SHIFT),
								     before_at, before);
		}
		if (fault && pages == 1 && test_bit(first, range->mapped) && !held && !moved_once) {
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
		/* fxdax: diagnostic (TASK-133): what the move will keep is what the window showed. */
		if (held && before && memcmp(held, before, PAGE_SIZE))
			pr_err("fxfs: ino %lu page %lu: held differs from the window's before the move: size %lld, seals %llu\n",
			       inode->i_ino, (start >> PAGE_SHIFT) + first, i_size_read(inode),
			       fxfs_device_seals(dax->dev));
		/* fxdax: fault injection, as fxfs_dax_map_write waiting for compaction's seal. */
		if (fault && READ_ONCE(fxfs_dax_fault_seals)) {
			err = fxfs_nofs(fxfs_fsync(fxfs_vfs_fs(inode->i_sb)));
			if (err)
				goto out;
		}
		/*
		 * fxdax: the PTEs of the pages go before their blocks move, for good until the move
		 * ends, so that a load faults, and waits. Moved first, a fault's blocks were freed from
		 * under what mapped where they were, a load through a PTE reading zeros once the old
		 * extent was, until the range mapped where they went: as a seal kept it from the gate
		 * (TASK-149.14), or as it stalled before the PTEs went (guest/sim, interleave 2376 at
		 * 1,000). A write(2)'s map the new blocks once MAPped, before its bytes are in, which
		 * a copy from them would read (TASK-150.8), or, gone through a region, lag, mapping the
		 * old blocks, which a PTE went on reading after the commit (TASK-153.3). First the move
		 * waits for the faults past their iomaps, which may yet map a page, as none after can,
		 * the file's semaphore held: they wait for nothing it holds. Not for every use of the
		 * range: an iomap waiting for the semaphore holds one (guest/sim, a deadlock).
		 */
		wait_var_event(&range->faulting, !atomic_read(&range->faulting));
		unmap_mapping_pages(inode->i_mapping, (start >> PAGE_SHIFT) + first, pages, false);
		/* As in fxfs_dax_map_chunk: unless a seal was in progress, or began since. */
		seals = fxfs_device_seals(dax->dev);
		/* A fault's pages hold what they held: a store may change any part of them. */
		began = ktime_get_ns();
		count = fxfs_dax_reserve_write(inode, pos, end, reserved, size, fault, held,
					     extents, capacity, timed ? &pending : NULL);
		if (timed)
			atomic64_add(ktime_get_ns() - began, &dax->write_fxfs_ns);
		fxfs_dax_delay();
		if (fault && count >= 0)
			fxfs_dax_pause(FXFS_DAX_STEP_MOVED);
		if (count < 0) {
			err = count;
			goto out;
		}
		/* fxdax: a write(2)'s blocks stay the file's until it commits: nothing moved yet. */
		moved = pending ? 0 : count;
		moved_once |= moved;
		/* fxdax: fault injection, a seal between the move and the mapping of the new block. */
		if (fault && fxfs_dax_seal_moved()) {
			err = fxfs_nofs(fxfs_fsync(fxfs_vfs_fs(inode->i_sb)));
			if (err)
				goto out;
		}
		began = ktime_get_ns();
		gate = fxfs_dax_store_begin(dax, fault);
		if (timed)
			atomic64_add(ktime_get_ns() - began, &dax->write_gate_ns);
		if (gate == seals) {
			bool fail = count > 1 && fxfs_dax_inject(&fxfs_dax_fail_maps);

			if (count == 1 && !fail &&
			    fxfs_dax_goes_through(range, extents, first, pages, flags) &&
			    !fxfs_dax_open_through(dax, file, extents, pos, end, &through)) {
				/*
				 * fxdax: a slice, whose region the blocks moved out of, falls behind
				 * them, for the file's next use to map it again: a slice of the region
				 * they went to, which the bytes go through (TASK-153.5). Moved before
				 * the bytes, the slice refused them, and the write went round again.
				 */
				if (range->region) {
					fxfs_dax_fall_behind(inode, range, -EXDEV);
					file->slice_left = range->chunk + 1;
					atomic64_inc(&dax->writes_through);
					break;
				}
				bitmap_set(range->lagging, first, pages);
				atomic64_inc(&dax->writes_through);
				break;
			}
			began = ktime_get_ns();
			err = fxfs_dax_map_extents(dax, range, extents, fail ? 1 : count);
			if (timed)
				atomic64_add(ktime_get_ns() - began, &dax->write_map_ns);
			if (!err && fail)
				err = -EIO;
			if (!err && before)
				fxfs_dax_check_moved(dax, inode, range, first, before, before_at,
						     before_where,
						     fault ? 0 : offset_in_page(pos),
						     fault ? 0 : min_t(loff_t, end - pos,
								       PAGE_SIZE - offset_in_page(pos)));
			if (err) {
				fxfs_device_store_end(dax->dev);
				/*
				 * fxdax: the blocks moved, but the range maps some where they were:
				 * dropped, and mapped again from where Fxfs has them (TASK-79.106). Or,
				 * a write(2)'s move dropped, below, it maps some where they would have
				 * gone.
				 */
				err = fxfs_dax_fall_behind(inode, range, err);
				moved = 0;
				goto out;
			}
			break;
		}
		/*
		 * fxdax: a seal came between the move and the gate. The blocks move again, from where
		 * Fxfs has them now, unless the gate is shut, for a fault, which gives up its page to
		 * wait for the seal, and with it the semaphore (`moved`, below). A write(2)'s move,
		 * whose new blocks the seal sealed, is dropped, which leaves them as they were.
		 */
		if (gate >= 0) {
			fxfs_device_store_end(dax->dev);
			if (pending) {
				fxfs_dax_drop_pending(dax, pending);
				pending = NULL;
			}
			if (fault)
				fxfs_dax_pause(FXFS_DAX_STEP_RESEALED);
			continue;
		}
		fxfs_dax_pause(FXFS_DAX_STEP_SHUT);
		err = gate;
		goto out;
	}
	kfree(held);
	kfree(before);
	kvfree(extents);
	if (!through)
		fxfs_dax_shadow_check(dax, range, first, pages, fault ? "a fault" : "a write");
	iomap->type = IOMAP_MAPPED;
	/* Holes a mapping of the file maps zeros for are gone. */
	iomap->flags |= IOMAP_F_PRIVATE | (fresh ? IOMAP_F_NEW : 0);
	iomap->addr = through ?: fxfs_dax_page_offset(range, first) + offset_in_page(pos);
	fxfs_dax_trace(fault ? FXFS_DAX_TRACE_FAULT : FXFS_DAX_TRACE_WRITE, inode->i_ino,
		       (start >> PAGE_SHIFT) + first, fxfs_dax_page_offset(range, first), 0, pages, 0);
	/* fxdax: a page's place within the window, or its fault dies of SIGBUS (TASK-121). */
	WARN(iomap->addr >= dax->window_len,
	     "fxfs: ino %lu page %lu written at %llx, past the window's %llx\n",
	     inode->i_ino, first, iomap->addr, dax->window_len);
	iomap->length = end - pos;
	iomap->private = range;
	/* fxdax: a fault's, until its iomap_end, as moves wait for it (`faulting`). */
	if (fault)
		atomic_inc(&range->faulting);
	/* fxdax: held until its bytes are in, and its move commits (fxfs_dax_iomap_end). */
	if (pending)
		file->pending = pending;
	else
		up_write(&file->sem);
	if (size)
		fxfs_dax_pause_file(FXFS_DAX_STEP_WRITE_MAPPED, inode->i_ino);
	return 0;
out:
	/*
	 * fxdax: blocks Fxfs moved that the range doesn't map yet, leaving: it maps them where they
	 * moved, read-only, whatever the way out. Else it goes on mapping them where they were, in an
	 * extent Fxfs frees, where the device puts zeros, which the next fault's copy of the page
	 * held: a seal shutting the gate as a fault went to map them (TASK-121), or as it went to
	 * move them again, a seal having ended meanwhile (TASK-133).
	 */
	if (moved) {
		int mapped = fxfs_dax_map_moved(dax, range, extents, moved);

		if (mapped)
			err = fxfs_dax_fall_behind(inode, range, mapped);
	}
	/* fxdax: a write(2)'s move, dropped, leaves the file as it was. */
	if (pending)
		fxfs_dax_drop_pending(dax, pending);
	up_write(&file->sem);
	kfree(held);
	kfree(before);
	kvfree(extents);
	fxfs_dax_put_range(dax, range);
	return err;
}

/*
 * fxdax: copying on write (doc-13): a write(2) newly making at least this many pages private asks
 * the device to make them so at once (PRIVATE), one round trip rather than a host fault each; 0
 * never.
 */
static unsigned int fxfs_dax_private_ahead = 4;
module_param_named(dax_private_ahead, fxfs_dax_private_ahead, uint, 0644);
MODULE_PARM_DESC(dax_private_ahead, "Copying on write, make a write's pages private ahead of its copy from this many on (0: never)");

/*
 * fxdax: copying on write, for tests (doc-13's stage 2): every this many milliseconds, the budget's
 * worker writes back, seals and rebinds every private page it can, under whatever stores go on; 0
 * never. Setting it starts it, on the filesystem mounted last.
 */
static unsigned int fxfs_dax_rebind_every;

/* fxdax: copying on write, a writeback's pages of a range: the dirty set it swapped out. */
struct fxfs_dax_cow_batch {
	struct fxfs_dax_range *range;
	unsigned long pages[BITS_TO_LONGS(FXFS_DAX_PAGES)];
};

/*
 * fxdax: the most pages one fxfs_write_many writes back: a megabyte, as one of the page cache's
 * writeback's (FXFS_IO_MAX, super.c).
 */
#define FXFS_DAX_COW_RUN 256

/*
 * fxdax: has Fxfs write the `nr` parts `parts` of file `ino`, `pages` dirty pages, paid for with their
 * room; or, with no part, of pages past the file's end, releases it.
 */
static int fxfs_dax_cow_write_run(struct fxfs *fs, u64 ino, const struct fxfs_write_part *parts,
				  size_t nr, unsigned long pages)
{
	if (nr)
		return fxfs_nofs(fxfs_write_many(fs, ino, parts, nr, (u64)pages << PAGE_SHIFT));
	if (pages)
		fxfs_release_dirty(fs, ino, (u64)pages << PAGE_SHIFT);
	return 0;
}

/*
 * fxdax: has Fxfs write the `nr` ranges' pages `batch`, of `inode`, from the window, where stores
 * made them private, to new extents, as the page cache's writeback writes folios
 * (fxfs_vfs_writepages): FXFS_DAX_COW_RUN pages a call at most, each with the room its dirtying
 * reserved, up to the file's end. Fxfs copies them into its transfer buffer, checksums that, and
 * commits after the bytes; a store meanwhile reaches the copy or not, its page dirty again either
 * way. `parts` has room for FXFS_DAX_COW_RUN. The first error, the rest written still.
 */
static int fxfs_dax_cow_write_pages(struct fxfs_dax *dax, struct inode *inode,
				    struct fxfs_dax_cow_batch *batch, size_t nr,
				    struct fxfs_write_part *parts)
{
	struct fxfs *fs = fxfs_vfs_fs(inode->i_sb);
	loff_t size = i_size_read(inode);
	unsigned long run = 0, page, end = 0;
	size_t i, n = 0;
	int err = 0, ran, id;

	id = dax_read_lock();
	for (i = 0; i < nr; i++) {
		struct fxfs_dax_range *range = batch[i].range;
		loff_t start = fxfs_dax_chunk_start(range);

		for (page = find_first_bit(batch[i].pages, range->pages); page < range->pages;
		     page = find_next_bit(batch[i].pages, range->pages, end)) {
			loff_t pos = start + ((loff_t)page << PAGE_SHIFT);
			void *kaddr = NULL;
			long had;

			end = min(find_next_zero_bit(batch[i].pages, range->pages, page),
				  page + FXFS_DAX_COW_RUN - run);
			had = dax_direct_access(dax->dax_dev,
						fxfs_dax_page_offset(range, page) >> PAGE_SHIFT,
						end - page, DAX_ACCESS, &kaddr, NULL);
			/* Past the file's end, as a write that copied nothing there leaves: nothing. */
			if (pos < size && had >= (long)(end - page))
				parts[n++] = (struct fxfs_write_part) {
					.offset = pos,
					.data = kaddr,
					.len = min_t(loff_t, (loff_t)(end - page) << PAGE_SHIFT,
						     size - pos),
				};
			else if (pos < size)
				err = err ?: -EIO;
			run += end - page;
			if (run == FXFS_DAX_COW_RUN) {
				ran = fxfs_dax_cow_write_run(fs, inode->i_ino, parts, n, run);
				err = err ?: ran;
				n = 0;
				run = 0;
			}
		}
	}
	ran = fxfs_dax_cow_write_run(fs, inode->i_ino, parts, n, run);
	dax_read_unlock(id);
	return err ?: ran;
}

/*
 * fxdax: copying on write, makes the user's mappings of the `nr` ranges' pages `batch`, a
 * writeback's, read-only, as the sealer does a file's (wp_shared_mapping_range): a store to one
 * after this faults, and dirties its page again. By the page tables, not the DAX entries: a fault
 * past its iomap, which made its page dirty and its PTE writable, holds the page's entry until it
 * returns, after its iomap_end let the semaphore go and a writeback swapped the page out; and
 * dax_writeback_mapping_range, finding the entry locked, gives up on it rather than wait
 * (dax_writeback_one's get_next_unlocked_entry, mid-walk, says it gone). That PTE stayed
 * writable: a rebind mapped the page read-only under it, which the host refuses a store to, or a
 * store through it after the writeback's copy went in no writeback (guest/sim, seed 22).
 */
static void fxfs_dax_cow_protect(struct address_space *mapping,
				 const struct fxfs_dax_cow_batch *batch, size_t nr)
{
	unsigned long page, end;
	size_t i;

	for (i = 0; i < nr; i++) {
		const struct fxfs_dax_range *range = batch[i].range;
		pgoff_t first = fxfs_dax_chunk_start(range) >> PAGE_SHIFT;

		for (page = find_first_bit(batch[i].pages, range->pages); page < range->pages;
		     page = find_next_bit(batch[i].pages, range->pages, end)) {
			end = find_next_zero_bit(batch[i].pages, range->pages, page);
			wp_shared_mapping_range(mapping, first + page, end - page);
		}
	}
}

/*
 * fxdax: copying on write, makes `file`'s dirty pages the VM's own on the host (PRIVATE), which
 * the set its writeback swaps out is: their blocks move to the extents the writeback allocates,
 * and the old die, which Fxfs frees after the next flush, the window reading zeros where it maps
 * them. A store made its page private as it copied what the window mapped; one an iomap dirtied
 * but whose store hadn't come, a fault's whose PTE the writeback write-protected
 * (fxfs_dax_cow_protect), still mapped its old block: the store, coming after its free, copied
 * zeros with it, which the next writeback wrote over the page (guest/sim, seed 2802 at 1,000
 * operations). No MAP over them follows until the writeback has committed (`wb_lock`, `users`).
 * The first error. With the file's semaphore held for writing, so that no page is dirtied
 * meanwhile.
 */
static int fxfs_dax_cow_own(struct fxfs_dax *dax, struct fxfs_dax_file *file)
{
	struct fxfs_dax_range *range;
	unsigned long chunk, page, end;
	int err;

	lockdep_assert_held_write(&file->sem);
	xa_for_each(&file->ranges, chunk, range) {
		if (!fxfs_dax_cow_busy(range))
			continue;
		for (page = find_first_bit(range->dirty, range->pages); page < range->pages;
		     page = find_next_bit(range->dirty, range->pages, end)) {
			end = find_next_zero_bit(range->dirty, range->pages, page);
			err = fxfs_device_private(dax->dev, fxfs_dax_page_offset(range, page),
						  (u64)(end - page) << PAGE_SHIFT);
			atomic64_inc(&dax->cow_owns);
			if (err)
				return err;
		}
	}
	return 0;
}

/*
 * fxdax: copying on write, writes back `inode`'s dirty pages (doc-13), as fsync, msync, sync, the
 * flusher (writepages), the budget, an UNMAP of a range and eviction need. Under its file's
 * semaphore held for writing, so that no write's iomap is between making a page writable and
 * its store, the dirty set is made the VM's own (fxfs_dax_cow_own), and swapped for an empty one:
 * the writeback set, which the writeback lock (`wb_lock`) keeps from rebinding until Fxfs has
 * committed it. Then what faults made writable is write-protected (fxfs_dax_cow_protect), so
 * that a store after faults, and dirties its page again, and the mapping's dirty entries cleaned
 * (dax_writeback_mapping_range, WB_SYNC_ALL, as the flusher's WB_SYNC_NONE does nothing); and the
 * set's bytes are written to new extents, which Fxfs allocates now, committing after them
 * (fxfs_dax_cow_write_pages). Swapped under the semaphore shared, a fault between its iomap and
 * its PTE left a writable PTE to a page the set took, which a rebind then lost the stores of.
 */
static int fxfs_dax_cow_write_back(struct inode *inode)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct writeback_control all = {
		.sync_mode = WB_SYNC_ALL,
		.nr_to_write = LONG_MAX,
		.range_start = 0,
		.range_end = LLONG_MAX,
	};
	struct fxfs_dax_cow_batch *batch = NULL;
	struct fxfs_write_part *parts = NULL;
	struct fxfs_dax_range *range;
	unsigned long chunk, word;
	size_t nr = 0, most = 0, i;
	long pages = 0;
	int err = 0, cleaned;

	if (!file)
		return 0;
	/*
	 * Whatever is dirty now: a writeback under way, the budget's or the flusher's, swapped out
	 * pages dirtied before this began, which it holds the lock over until Fxfs has committed
	 * them, and an fsync returns only once they are durable. Returning at once, as no page was
	 * dirty, an fsync met by the worker's writeback sealed none of its pages (guest/sim, seed 962).
	 */
	mutex_lock(&file->wb_lock);
	if (!atomic_long_read(&file->dirty_pages))
		goto out;
	down_read(&file->sem);
	xa_for_each(&file->ranges, chunk, range)
		most += fxfs_dax_cow_busy(range);
	up_read(&file->sem);
	if (!most)
		goto out;
	/* Before the swap: a writeback that can't write leaves the set dirty. */
	batch = fxfs_nofs(kvmalloc_array(most, sizeof(*batch), GFP_KERNEL));
	parts = kmalloc_array(FXFS_DAX_COW_RUN, sizeof(*parts), GFP_NOFS);
	if (!batch || !parts) {
		err = -ENOMEM;
		goto out;
	}
	down_write(&file->sem);
	/* Its pages the VM's own before they are the set's: one that can't be stays dirty. */
	err = fxfs_dax_cow_own(dax, file);
	if (err) {
		up_write(&file->sem);
		goto out;
	}
	xa_for_each(&file->ranges, chunk, range) {
		if (nr == most || !fxfs_dax_cow_busy(range))
			continue;
		batch[nr].range = range;
		/* Kept, mapped, while the writeback reads it: no reclaim, no drop. */
		atomic_inc(&range->users);
		bitmap_zero(batch[nr].pages, FXFS_DAX_PAGES);
		for (word = 0; word < BITS_TO_LONGS(range->pages); word++) {
			batch[nr].pages[word] = range->dirty[word];
			range->dirty[word] = 0;
		}
		pages += bitmap_weight(batch[nr].pages, range->pages);
		nr++;
	}
	fxfs_dax_cow_count(dax, file, 0, -pages);
	up_write(&file->sem);
	fxfs_dax_cow_protect(inode->i_mapping, batch, nr);
	cleaned = dax_writeback_mapping_range(inode->i_mapping, dax->dax_dev, &all);
	/* Before it reads its ranges' window pages, which nothing may free (pause points). */
	fxfs_dax_pause_file(FXFS_DAX_STEP_WRITEBACK, inode->i_ino);
	err = fxfs_dax_cow_write_pages(dax, inode, batch, nr, parts);
	err = err ?: cleaned;
	for (i = 0; i < nr; i++)
		fxfs_dax_put_range(dax, batch[i].range);
	atomic64_inc(&dax->cow_writebacks);
	atomic64_add(pages, &dax->cow_written);
	/* As the page cache's writeback: the pages are clean, the error the file's (fsync's). */
	if (err)
		mapping_set_error(inode->i_mapping, err);
out:
	mutex_unlock(&file->wb_lock);
	kfree(parts);
	kvfree(batch);
	return err;
}

/*
 * fxdax: rebinds `file`'s clean private pages (doc-13): those in no dirty set, nor in a writeback's
 * under way, which the writeback lock waits for, each MAPped to the sealed extent's page holding
 * its bytes, as Fxfs has them, or to zeros where it has none: the same bytes at the same address,
 * the host's private page freed, nothing in the guest changed. Under the file's semaphore held for
 * writing, which sees each with no writable PTE (the fault that made one dirtied its page, which
 * its writeback write-protected) and no copy into it under way (invariant 3). A page whose
 * extent is open yet, its writeback's barrier not come, stays private. How many it rebound.
 */
static long fxfs_dax_cow_rebind(struct fxfs_dax *dax, struct fxfs_dax_file *file)
{
	struct fxfs_dax_volume *volume;
	struct fxfs_dax_range *range;
	unsigned long chunk, page, end;
	long rebound = 0, done;

	if (!atomic_long_read(&file->private_pages))
		return 0;
	mutex_lock(&file->wb_lock);
	down_write(&file->sem);
	/* A volume going (fxfs_dax_remove_volume) stops the worker first. */
	volume = READ_ONCE(file->volume);
	xa_for_each(&file->ranges, chunk, range) {
		if (!volume || !range->private || range->behind)
			continue;
		for (page = find_first_bit(range->private, range->pages); page < range->pages;
		     page = find_next_bit(range->private, range->pages, end)) {
			end = page + 1;
			if (test_bit(page, range->dirty))
				continue;
			while (end < range->pages && test_bit(end, range->private) &&
			       !test_bit(end, range->dirty))
				end++;
			/* Before the MAP over them, holding what a rebind holds (pause points). */
			fxfs_dax_pause_file(FXFS_DAX_STEP_REBIND, file->ino);
			done = fxfs_dax_cow_map(dax, volume->fs, range, page, end, true);
			if (done < 0)
				pr_warn_ratelimited("fxfs: rebinding ino %llu chunk %lu pages %lu to %lu: %ld\n",
						    file->ino, chunk, page, end, done);
			else
				rebound += done;
		}
	}
	up_write(&file->sem);
	mutex_unlock(&file->wb_lock);
	atomic64_add(rebound, &dax->cow_rebound);
	return rebound;
}

/*
 * fxdax: references to every volume's files with dirty or private pages, in `*files`, which the
 * caller frees, putting each: how many.
 */
static size_t fxfs_dax_cow_files(struct fxfs_dax *dax, struct fxfs_dax_file ***files)
{
	struct fxfs_dax_volume *volume;
	struct fxfs_dax_file *file;
	size_t nr = 0, most = 0;
	unsigned long ino;

	*files = NULL;
	mutex_lock(&dax->files_lock);
	fxfs_dax_for_each_file(dax, volume, ino, file)
		most += atomic_long_read(&file->dirty_pages) ||
			atomic_long_read(&file->private_pages);
	if (most)
		*files = fxfs_nofs(kvmalloc_array(most, sizeof(**files), GFP_KERNEL));
	if (*files) {
		fxfs_dax_for_each_file(dax, volume, ino, file) {
			if (nr < most && (atomic_long_read(&file->dirty_pages) ||
					  atomic_long_read(&file->private_pages))) {
				refcount_inc(&file->refs);
				(*files)[nr++] = file;
			}
		}
	}
	mutex_unlock(&dax->files_lock);
	return nr;
}

/* fxdax: the inode read in for `file`, with a reference, if there is one. */
static struct inode *fxfs_dax_cow_inode(struct fxfs_dax *dax, struct fxfs_dax_file *file)
{
	struct inode *inode = NULL;

	mutex_lock(&dax->files_lock);
	if (file->inode)
		inode = igrab(file->inode);
	mutex_unlock(&dax->files_lock);
	return inode;
}

/*
 * fxdax: relieves the private pages (doc-13's budget): writes back every file with dirty pages,
 * if `write`, not while frozen; seals what that wrote, for its pages' extents to map; and rebinds
 * clean private pages, file by file, until no more than `target` are private.
 */
static void fxfs_dax_cow_relieve(struct fxfs_dax *dax, long target, bool write)
{
	struct fxfs_dax_file **files;
	size_t nr = fxfs_dax_cow_files(dax, &files), i;
	struct fxfs *wrote = NULL;
	int err;

	for (i = 0; write && i < nr; i++) {
		struct fxfs_dax_volume *volume = READ_ONCE(files[i]->volume);
		struct inode *inode;

		if (!volume || !atomic_long_read(&files[i]->dirty_pages))
			continue;
		inode = fxfs_dax_cow_inode(dax, files[i]);
		if (!inode)
			continue;
		err = fxfs_dax_cow_write_back(inode);
		if (err)
			pr_warn_ratelimited("fxfs: writing back ino %lu's private pages: %d\n",
					    inode->i_ino, err);
		iput(inode);
		wrote = volume->fs;
	}
	/* The barrier after them, which the journal's flush makes, seals them. */
	if (wrote) {
		atomic64_inc(&dax->cow_seals);
		err = fxfs_nofs(fxfs_fsync(wrote));
		if (err)
			pr_warn_ratelimited("fxfs: sealing what the budget wrote back: %d\n", err);
	}
	for (i = 0; i < nr; i++) {
		if (atomic_long_read(&dax->private) > target)
			fxfs_dax_cow_rebind(dax, files[i]);
		fxfs_dax_put_file(dax, files[i]);
	}
	kvfree(files);
}

/*
 * fxdax: the seal count at which what was written before now is sealed: that of the end of the
 * first barrier begun after now. One in progress (odd) may have been sent before: the next.
 */
static u64 fxfs_dax_seal_target(struct fxfs_dax *dax)
{
	u64 seals = fxfs_device_seals(dax->dev);

	return seals + 2 + (seals & 1);
}

/* fxdax: forgets the page-cache inodes of the volumes' `unsealed` whose barrier has come. */
static void fxfs_dax_cow_prune(struct fxfs_dax *dax)
{
	u64 seals = fxfs_device_seals(dax->dev);
	struct fxfs_dax_volume *volume;
	unsigned long ino;
	void *entry;

	if (!atomic_long_read(&dax->unsealed))
		return;
	mutex_lock(&dax->files_lock);
	list_for_each_entry(volume, &dax->volumes, list) {
		xa_for_each(&volume->unsealed, ino, entry) {
			if (seals >= xa_to_value(entry) &&
			    xa_cmpxchg(&volume->unsealed, ino, entry, NULL, GFP_NOFS) == entry)
				atomic_long_dec(&dax->unsealed);
		}
	}
	mutex_unlock(&dax->files_lock);
}

/*
 * fxdax: the budget's worker (doc-13): past seven eighths of private_max, it writes back dirty
 * files and rebinds clean private pages, to three quarters; asked to (`cow_clean`, as the window
 * runs out of room for ranges), it writes back whatever is dirty; asked to rebind all
 * (`cow_rebind_all`, fxfs.dax_rebind_every), as many as it can. Then it prunes the page-cache
 * inodes sealed since, and wakes those the budget held up, and the window's reclaim.
 */
static void fxfs_dax_cow_work(struct work_struct *work)
{
	struct fxfs_dax *dax = container_of(work, struct fxfs_dax, cow_work);
	unsigned int every = READ_ONCE(fxfs_dax_rebind_every);
	bool clean = atomic_cmpxchg(&dax->cow_clean, 1, 0) == 1;
	bool all = atomic_cmpxchg(&dax->cow_rebind_all, 1, 0) == 1 || every;
	long max = dax->private_max;

	if (clean || all || atomic_long_read(&dax->private) > max - max / 8)
		fxfs_dax_cow_relieve(dax, all ? 0 : max - max / 4, !READ_ONCE(dax->frozen));
	fxfs_dax_cow_prune(dax);
	atomic64_inc(&dax->cow_rounds);
	wake_up_all(&dax->cow_wait);
	fxfs_dax_released(dax);
	if (every && !READ_ONCE(dax->cow_stopping)) {
		msleep(every);
		fxfs_dax_cow_kick(dax, false);
	}
}

/*
 * fxdax: counts `pages` more pages private, as a write's iomap makes them writable, if private_max
 * has room for them; else none, and false, for the iomap to wait for the worker (doc-13's budget).
 * Past seven eighths of it, the worker writes back and rebinds.
 */
static bool fxfs_dax_cow_take(struct fxfs_dax *dax, long pages)
{
	long now = atomic_long_add_return(pages, &dax->private);

	if (now > dax->private_max) {
		atomic_long_sub(pages, &dax->private);
		atomic64_inc(&dax->cow_throttled);
		fxfs_dax_cow_kick(dax, false);
		return false;
	}
	if (now > dax->private_max - dax->private_max / 8)
		fxfs_dax_cow_kick(dax, false);
	return true;
}

/*
 * fxdax: waits, holding nothing of the file's, for a round of the worker's since a write's iomap
 * found no room among the private pages (-ENOBUFS), as balance_dirty_pages waits after a fault: 0
 * to try again, -EINTR if killed, or -ENOMEM once FXFS_DAX_NO_ROOM_WAITS rounds or waits of 10 ms
 * made none, as for a range.
 */
static int fxfs_dax_cow_throttle(struct fxfs_dax *dax, unsigned int *waits)
{
	s64 round = atomic64_read(&dax->cow_rounds);
	long waited;

	fxfs_dax_cow_kick(dax, false);
	waited = wait_event_killable_timeout(dax->cow_wait,
					     atomic64_read(&dax->cow_rounds) != round,
					     msecs_to_jiffies(10));
	if (waited < 0)
		return -EINTR;
	if (++*waits > FXFS_DAX_NO_ROOM_WAITS)
		return -ENOMEM;
	return 0;
}

/* fxdax: whether `range`'s page `page` maps zeros, written never: a hole, or past the file's end. */
static bool fxfs_dax_cow_hole(const struct fxfs_dax_range *range, unsigned long page)
{
	return test_bit(page, range->zeros) && !test_bit(page, range->private);
}

/*
 * fxdax: copying on write, maps a write of up to `length` bytes at `pos`, a fault's, a write(2)'s or
 * zeroing's, at its pages' own places in the window (doc-13), where the store makes each private on
 * the host, holding what it held: no MAP, no transaction, nothing moved. Its pages join the file's
 * dirty set, with room reserved in Fxfs for their writeback (a fault without it fails SIGBUS, a
 * write(2) ENOSPC, as the page cache's do; zeroing, a truncate's or a punch's, takes what there
 * is), and count private, within private_max: else -ENOBUFS, for the caller to wait for the budget's
 * worker, holding nothing (fxfs_dax_cow_throttle). Returns holding the file's semaphore shared until
 * fxfs_dax_iomap_end: no writeback swaps the dirty set out, nor rebinding maps a page over, while
 * the store or the copy goes on. A slice's chunk, whose pages its region's other slices map, or a
 * range too small, is dropped (-EXDEV), and mapped again as a range of its own, as large as the
 * write needs. Zeroing leaves holes, and pages of zeros never written, be.
 */
static int fxfs_dax_cow_iomap_write(struct inode *inode, loff_t pos, loff_t length,
				    unsigned int flags, struct iomap *iomap)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs *fs = fxfs_vfs_fs(inode->i_sb);
	bool fault = flags & IOMAP_FAULT, zero = flags & IOMAP_ZERO;
	loff_t start, end = min_t(loff_t, pos + length, round_up(pos + 1, FXFS_DAX_SIZE));
	long fresh = 0, newly = 0, dirtied = 0, privated = 0;
	unsigned long first, pages, page, to;
	struct fxfs_dax_range *range;
	unsigned int ahead;
	int err;

	range = fxfs_dax_get_range_timed(inode, pos >> FXFS_DAX_SHIFT, fault || zero ? 0 : end, 0,
					 fault, false, false);
	if (IS_ERR(range))
		return PTR_ERR(range);
	start = fxfs_dax_chunk_start(range);
	if (range->region || start + ((loff_t)range->pages << PAGE_SHIFT) <= pos) {
		fxfs_dax_put_range(dax, range);
		return -EXDEV;
	}
	first = (pos - start) >> PAGE_SHIFT;
	end = min_t(loff_t, end, start + ((loff_t)min(range->pages, first + dax->cow_pages) <<
					  PAGE_SHIFT));
	pages = DIV_ROUND_UP(end - start, PAGE_SIZE) - first;
	for (;;) {
		err = fxfs_dax_down_read(file, fault);
		if (err)
			goto put;
		if (range->behind) {
			err = range->behind;
			goto unlock;
		}
		if (find_next_zero_bit(range->mapped, first + pages, first) == first + pages)
			break;
		/* A page a truncate's MAP of zeros failed to map (fxfs_dax_resize_end): mapped now. */
		up_read(&file->sem);
		err = fxfs_dax_down_write(file, fault);
		if (err)
			goto put;
		while (!err &&
		       (page = find_next_zero_bit(range->mapped, first + pages, first)) <
		       first + pages) {
			long mapped;

			to = find_next_bit(range->mapped, first + pages, page);
			mapped = fxfs_dax_cow_map(dax, fs, range, page, to, false);
			err = mapped < 0 ? mapped : 0;
		}
		up_write(&file->sem);
		if (err)
			goto put;
	}
	if (zero) {
		bool hole = fxfs_dax_cow_hole(range, first);

		for (page = first + 1; page < first + pages && fxfs_dax_cow_hole(range, page) == hole;
		     page++)
			;
		pages = page - first;
		end = min_t(loff_t, end, start + ((loff_t)page << PAGE_SHIFT));
		if (hole) {
			up_read(&file->sem);
			fxfs_dax_put_range(dax, range);
			fxfs_dax_hole(iomap, end - pos);
			return 0;
		}
	}
	for (page = first; page < first + pages; page++) {
		fresh += !test_bit(page, range->dirty);
		newly += !test_bit(page, range->private);
	}
	if (fresh && zero) {
		unsigned int nofs = memalloc_nofs_save();

		fxfs_reserve_dirty_what_fits(fs, inode->i_ino, (u64)fresh << PAGE_SHIFT);
		memalloc_nofs_restore(nofs);
	} else if (fresh) {
		err = fxfs_nofs(fxfs_reserve_dirty(fs, inode->i_ino, (u64)fresh << PAGE_SHIFT));
		if (err)
			goto unlock;
	}
	if (newly && !fxfs_dax_cow_take(dax, newly)) {
		err = -ENOBUFS;
		goto unreserve;
	}
	for (page = first; page < first + pages; page++) {
		privated += !test_and_set_bit(page, range->private);
		dirtied += !test_and_set_bit(page, range->dirty);
	}
	/* Others' iomaps, holding the semaphore shared too, may have counted some meanwhile. */
	if (newly > privated)
		atomic_long_sub(newly - privated, &dax->private);
	atomic_long_add(privated, &file->private_pages);
	if (fresh > dirtied)
		fxfs_release_dirty(fs, inode->i_ino, (u64)(fresh - dirtied) << PAGE_SHIFT);
	if (dirtied) {
		atomic_long_add(dirtied, &dax->dirty);
		atomic_long_add(dirtied, &file->dirty_pages);
		/* For the flusher's writepages: a write(2)'s pages tag nothing in the mapping. */
		__mark_inode_dirty(inode, I_DIRTY_PAGES);
	}
	/* A write(2) of many pages not yet private: one round trip, rather than a host fault each. */
	ahead = READ_ONCE(fxfs_dax_private_ahead);
	if (!fault && !zero && ahead && privated >= ahead) {
		int sent = fxfs_device_private(dax->dev, fxfs_dax_page_offset(range, first),
					       (u64)pages << PAGE_SHIFT);

		atomic64_inc(&dax->cow_privates);
		if (sent)
			pr_warn_ratelimited("fxfs: making %lu pages at %llx of the window private: %d\n",
					    pages, fxfs_dax_page_offset(range, first), sent);
	}
	iomap->type = IOMAP_MAPPED;
	iomap->addr = fxfs_dax_page_offset(range, first) + offset_in_page(pos);
	iomap->length = end - pos;
	iomap->private = range;
	/* fxdax: a page's place within the window, or its fault dies of SIGBUS (TASK-121). */
	WARN(iomap->addr >= dax->window_len,
	     "fxfs: ino %lu page %lu written at %llx, past the window's %llx\n",
	     inode->i_ino, first, iomap->addr, dax->window_len);
	fxfs_dax_trace(fault ? FXFS_DAX_TRACE_FAULT : FXFS_DAX_TRACE_WRITE, inode->i_ino,
		       (start >> PAGE_SHIFT) + first, fxfs_dax_page_offset(range, first), 0, pages, 0);
	/* Before the copy, its pages dirty, holding the semaphore shared (pause points). */
	if (!fault && !zero)
		fxfs_dax_pause_file(FXFS_DAX_STEP_WRITE_MAPPED, inode->i_ino);
	return 0;
unreserve:
	if (fresh)
		fxfs_release_dirty(fs, inode->i_ino, (u64)fresh << PAGE_SHIFT);
unlock:
	up_read(&file->sem);
put:
	fxfs_dax_put_range(dax, range);
	return err;
}

/*
 * fxdax: copying on write, rebinds every private page it can, as freezing does (fxfs_dax_frozen),
 * or as writing to fxfs.dax_rebind_now asks, on the filesystem mounted last: written back, sealed
 * and rebound, by the worker, waited for.
 */
static int fxfs_dax_rebind_now_set(const char *val, const struct kernel_param *kp)
{
	struct fxfs_dax *dax = READ_ONCE(fxfs_dax_stray_dax);

	if (!dax || !dax->cow)
		return -EINVAL;
	atomic_set(&dax->cow_rebind_all, 1);
	fxfs_dax_cow_kick(dax, true);
	flush_work(&dax->cow_work);
	return 0;
}

static const struct kernel_param_ops fxfs_dax_rebind_now_ops = { .set = fxfs_dax_rebind_now_set };
module_param_cb(dax_rebind_now, &fxfs_dax_rebind_now_ops, NULL, 0200);
MODULE_PARM_DESC(dax_rebind_now, "Copying on write, write back, seal and rebind every private page now, on the filesystem mounted last (tests)");

/* fxdax: fxfs.dax_rebind_every, set, starts the worker's rounds on the filesystem mounted last. */
static int fxfs_dax_rebind_every_set(const char *val, const struct kernel_param *kp)
{
	struct fxfs_dax *dax = READ_ONCE(fxfs_dax_stray_dax);
	int err = param_set_uint(val, kp);

	if (!err && dax && dax->cow && READ_ONCE(fxfs_dax_rebind_every))
		fxfs_dax_cow_kick(dax, false);
	return err;
}

static const struct kernel_param_ops fxfs_dax_rebind_every_ops = {
	.set = fxfs_dax_rebind_every_set,
	.get = param_get_uint,
};
module_param_cb(dax_rebind_every, &fxfs_dax_rebind_every_ops, &fxfs_dax_rebind_every, 0644);
MODULE_PARM_DESC(dax_rebind_every, "Copying on write, write back, seal and rebind every private page every this many ms (tests; 0: never)");

/*
 * fxdax: fault injection (TASK-164.2): the next this many of Fxfs's write_many transactions fail
 * once their blocks are written, before they commit, as a crash between the two would leave them
 * (fxfs_set_fail_commits).
 */
static unsigned long fxfs_dax_fail_commits;

static int fxfs_dax_fail_commits_set(const char *val, const struct kernel_param *kp)
{
	int err = param_set_ulong(val, kp);

	if (!err)
		fxfs_set_fail_commits(fxfs_dax_fail_commits);
	return err;
}

static const struct kernel_param_ops fxfs_dax_fail_commits_ops = {
	.set = fxfs_dax_fail_commits_set,
	.get = param_get_ulong,
};
module_param_cb(dax_fail_commits, &fxfs_dax_fail_commits_ops, &fxfs_dax_fail_commits, 0644);
MODULE_PARM_DESC(dax_fail_commits, "Fail the next this many write_many transactions after their bytes, before they commit (fault injection)");


static int fxfs_dax_iomap_begin(struct inode *inode, loff_t pos, loff_t length,
				unsigned int flags, struct iomap *iomap, struct iomap *srcmap)
{
	u64 began = ktime_get_ns();
	int err;

	/*
	 * fxdax: an iomap a page at most for a write of a file in pieces: a write of many, each with
	 * its tree lookups, held its CPU for 22 s, no one else getting it, without preemption
	 * (xfstests generic/111, TASK-140).
	 */
	cond_resched();
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
		/*
		 * fxdax: nor past an iomap whose bytes aren't the file's: dax_iomap_rw goes on after
		 * a copy cut short, which wrote the rest after a gap (fxfs_dax_committed).
		 */
		if (!(flags & (IOMAP_FAULT | IOMAP_ZERO)) && fxfs_dax_file(inode)->unwritten_err)
			return fxfs_dax_file(inode)->unwritten_err;
		/* fxdax: copying on write, a store makes its page private, nothing moved (doc-13). */
		if (fxfs_dax_of(inode)->cow)
			err = fxfs_dax_cow_iomap_write(inode, pos, length, flags, iomap);
		else
			err = fxfs_dax_iomap_write(inode, pos, length, flags, iomap, srcmap);
		if (!(flags & (IOMAP_FAULT | IOMAP_ZERO)))
			atomic64_add(ktime_get_ns() - began, &fxfs_dax_of(inode)->write_begin_ns);
		return err;
	}
	err = fxfs_dax_iomap_read(inode, pos, length, flags, iomap, srcmap);
	/* fxdax: between the iomap and the read of what it maps, which its range keeps mapped. */
	if (!err) {
		fxfs_dax_pause_file(FXFS_DAX_STEP_READ_MAPPED, inode->i_ino);
		fxfs_dax_delay();
	}
	return err;
}

static int fxfs_dax_iomap_end(struct inode *inode, loff_t pos, loff_t length, ssize_t written,
			      unsigned int flags, struct iomap *iomap)
{
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	bool written_by_write = (flags & IOMAP_WRITE) && !(flags & (IOMAP_FAULT | IOMAP_ZERO));
	struct fxfs_dax_write *pending = written_by_write ? file->pending : NULL;
	u64 began = ktime_get_ns();
	int err = 0;

	/*
	 * fxdax: copying on write, a write's iomap held the file's semaphore shared, which goes now
	 * its store or copy is done (fxfs_dax_cow_iomap_write): a write(2) past the file's end grows
	 * it by what it wrote, as below.
	 */
	if (dax->cow && (flags & (IOMAP_WRITE | IOMAP_ZERO))) {
		if (written_by_write && written > 0 && pos + written > i_size_read(inode)) {
			i_size_write(inode, pos + written);
			__mark_inode_dirty(inode, I_DIRTY_SYNC | I_DIRTY_DATASYNC);
		}
		if (iomap->private) {
			up_read(&file->sem);
			fxfs_dax_put_range(dax, iomap->private);
		}
		/* A fault's PTE in, its page's DAX entry held still (pause points). */
		if (flags & IOMAP_FAULT)
			fxfs_dax_pause_file(FXFS_DAX_STEP_FAULTED, inode->i_ino);
		if (written_by_write)
			atomic64_add(ktime_get_ns() - began, &dax->write_end_ns);
		return 0;
	}

	/*
	 * A write past the file's end grows it as it ends, by what it wrote, not as it begins: a
	 * reader finding the file longer reads what the write wrote there, never the zeros before
	 * it (TASK-133), as ext4's and XFS's DAX writes grow theirs. In the gate still, so that a
	 * seal takes the size with the bytes. An append's size goes with the inode's next writeback,
	 * not a transaction of its own for every write (TASK-92.28). fxdax: a write(2) whose move
	 * commits now, with the size, grows it once that has, below.
	 */
	if (!pending && written_by_write && written > 0 && pos + written > i_size_read(inode)) {
		i_size_write(inode, pos + written);
		__mark_inode_dirty(inode, I_DIRTY_SYNC | I_DIRTY_DATASYNC);
	}
	if (iomap->flags & IOMAP_F_PRIVATE)
		fxfs_device_store_end(dax->dev);
	/*
	 * fxdax: a write(2)'s move commits, now its bytes are in, out of the gate, as a commit may
	 * wait for a seal. A copy cut short drops it instead, leaving the file as it was, and so
	 * does a failed commit: the range, mapping the new blocks, falls behind them, and none of the
	 * iomap's bytes are the file's (fxfs_dax_committed). Then the file's semaphore goes, which
	 * kept faults from mapping the file meanwhile (TASK-153.5).
	 */
	if (pending) {
		file->pending = NULL;
		if (written == length)
			err = fxfs_nofs(fxfs_dax_end_write(pending, true));
		else
			fxfs_dax_drop_pending(dax, pending);
		if (written != length || err) {
			struct fxfs_dax_range *range = iomap->private;

			/* A slice can't map the blocks but where they are: -EXDEV, to be dropped. */
			fxfs_dax_fall_behind(inode, range, range->region ? -EXDEV : -ESTALE);
			file->unwritten = pos;
			file->unwritten_err = err ?: -EFAULT;
		} else if (pos + written > i_size_read(inode)) {
			i_size_write(inode, pos + written);
			__mark_inode_dirty(inode, I_DIRTY_SYNC | I_DIRTY_DATASYNC);
		}
		up_write(&file->sem);
	}
	if (written_by_write && file->through) {
		fxfs_dax_put_region(dax, file->through);
		file->through = NULL;
	}
	if (iomap->private) {
		struct fxfs_dax_range *range = iomap->private;

		if ((flags & IOMAP_FAULT) && atomic_dec_and_test(&range->faulting))
			wake_up_var(&range->faulting);
		fxfs_dax_put_range(dax, range);
	}
	if (written_by_write)
		atomic64_add(ktime_get_ns() - began, &dax->write_end_ns);
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
 * fxdax: what a way in to a file has of the MAPs refused for want of the host's memory: how many it
 * has seen, and how many seals it has made for them since it last got on.
 */
struct fxfs_dax_room {
	s64 refused;
	unsigned int seals;
};

static struct fxfs_dax_room fxfs_dax_room(struct fxfs_dax *dax)
{
	return (struct fxfs_dax_room) { .refused = atomic64_read(&dax->maps_refused) };
}

/*
 * fxdax: what follows a try that failed with `err`: 0 if that wasn't the host refusing a MAP for
 * want of memory since `room` last saw, 1 once a seal has made room for another try, and otherwise
 * what to fail with. The host can't evict an open extent's pages, nor seal them, and left to Fxfs's
 * journal a seal may be far off, each write of an open extent's blocks taking more of them: so it
 * refuses a MAP while memory is left, and what was refused seals, with no iomap held,
 * nor a DAX entry, which a store in the gate a seal waits for may be waiting for; in the gate, a MAP
 * can't wait for it. Whatever way the failure took: as the range fell behind (-ESTALE), a slice as
 * one is (-EXDEV), or it had no range at all (-ENOMEM). Those are dropped as they would be. A
 * filesystem that seals in vain, its host's memory another's, says ENOMEM, which is the truth of
 * it: never EIO (TASK-150.20).
 */
static int fxfs_dax_make_memory(struct inode *inode, struct fxfs_dax_room *room, int err)
{
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	s64 refused = atomic64_read(&dax->maps_refused);

	if ((err != -ENOMEM && err != -ESTALE && err != -EXDEV) || refused == room->refused ||
	    fxfs_vfs_follows(inode->i_sb))
		return 0;
	room->refused = refused;
	if (++room->seals > FXFS_DAX_MAX_DROPS)
		return -ENOMEM;
	atomic64_inc(&dax->memory_seals);
	return fxfs_nofs(fxfs_fsync(fxfs_vfs_fs(inode->i_sb))) ?: 1;
}

/*
 * fxdax: a read, whose copy takes no page fault: a read into a mapping of the file it reads holds
 * the range through its copy, and the fault waited for that range's users, itself among them, for
 * ever. It faults the destination in with no iomap held, as a write does its source.
 */
static ssize_t fxfs_dax_read_faulting_between(struct kiocb *iocb, struct iov_iter *to)
{
	struct inode *inode = file_inode(iocb->ki_filp);
	struct fxfs_dax_room room = fxfs_dax_room(fxfs_dax_of(inode));
	ssize_t ret, done = 0;
	unsigned int empty = 0, stale = 0;
	int sealed;

	for (;;) {
		pagefault_disable();
		ret = fxfs_dax_read(iocb, to);
		pagefault_enable();
		sealed = ret < 0 ? fxfs_dax_make_memory(inode, &room, ret) : 0;
		if (ret > 0) {
			done += ret;
			empty = 0;
			room.seals = 0;
		} else if (sealed) {
			if (sealed < 0) {
				ret = sealed;
				break;
			}
			if (ret != -ENOMEM) {
				ret = fxfs_dax_drop_slice(inode, iocb->ki_pos, true);
				if (ret)
					break;
			}
			continue;
		} else if (ret == -ESTALE || ret == -EXDEV) {
			/* fxdax: a range behind Fxfs's blocks, once: dropped, mapped again. */
			if (ret == -ESTALE && stale++) {
				ret = -EIO;
				break;
			}
			ret = fxfs_dax_drop_slice(inode, iocb->ki_pos, true);
			if (ret)
				break;
			continue;
		} else if (ret != -EFAULT && ret != 0) {
			break;
		}
		if (!iov_iter_count(to) || iocb->ki_pos >= i_size_read(inode))
			break;
		/* Nothing copied, memory there, and again: as a write's source, it won't get on. */
		if (ret <= 0 && ++empty > FXFS_DAX_MAX_DROPS) {
			ret = -EFAULT;
			break;
		}
		if (fault_in_iov_iter_writeable(to, iov_iter_count(to)) == iov_iter_count(to)) {
			ret = -EFAULT;
			break;
		}
	}
	return done ?: ret;
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
		ret = fxfs_dax_read_faulting_between(iocb, to);
	inode_unlock_shared(inode);
	file_accessed(iocb->ki_filp);
	return ret;
}

/* fxdax: bytes of a write, in kernel memory, for as long as it is writing them. */
struct fxfs_dax_source {
	struct kvec kvec;
	struct iov_iter iter;
};

/*
 * fxdax: takes the next of a write's bytes at `pos`, as many as reach the chunk's end, from `from`
 * into `source`, which the write then copies from, a copy having stopped short
 * (fxfs_dax_write_iter). With page faults enabled, holding no iomap and outside the gate, so that a
 * fault on a mapping of the file itself gets what it needs. -EFAULT if the user's memory holds none
 * of them; if it holds fewer, those are what the write writes.
 */
static int fxfs_dax_source_take(struct fxfs_dax_source *source, struct iov_iter *from, loff_t pos)
{
	size_t len = min_t(loff_t, iov_iter_count(from), round_up(pos + 1, FXFS_DAX_SIZE) - pos);

	/* The first's as large as any next: what's left of a write only goes down. */
	if (!source->kvec.iov_base)
		source->kvec.iov_base = fxfs_nofs(kvmalloc(min_t(size_t, iov_iter_count(from),
								 FXFS_DAX_SIZE), GFP_KERNEL));
	if (!source->kvec.iov_base)
		return -ENOMEM;
	source->kvec.iov_len = copy_from_iter(source->kvec.iov_base, len, from);
	if (!source->kvec.iov_len)
		return -EFAULT;
	iov_iter_kvec(&source->iter, ITER_SOURCE, &source->kvec, 1, source->kvec.iov_len);
	return 0;
}

/* fxdax: adds to `into` the time since `*at`, and moves `*at` on. */
static void fxfs_dax_lap(atomic64_t *into, u64 *at)
{
	u64 now = ktime_get_ns();

	atomic64_add(now - *at, into);
	*at = now;
}

/*
 * fxdax: where write_ns went (TASK-146.2), each part apart, so that they add up: dax_write_begin_ns
 * less the waits within it that have counters of their own, the copy, which is dax_iomap_rw less
 * its iomaps', and what none of them holds, which a new step in the write would show as.
 */
static void fxfs_dax_show_write_times(struct seq_file *m, struct fxfs_dax *dax)
{
	s64 begin = atomic64_read(&dax->write_begin_ns), end = atomic64_read(&dax->write_end_ns);
	s64 rw = atomic64_read(&dax->write_rw_ns), setup = atomic64_read(&dax->write_setup_ns);
	s64 fault_in = atomic64_read(&dax->write_fault_in_ns);
	s64 retry = atomic64_read(&dax->write_retry_ns), tail = atomic64_read(&dax->write_tail_ns);
	s64 lock = atomic64_read(&dax->write_lock_ns);
	s64 prepare = atomic64_read(&dax->write_prepare_ns);
	s64 range = atomic64_read(&dax->write_range_ns);

	seq_printf(m, "dax_write_setup_ns %lld\ndax_write_fault_in_ns %lld\n", setup, fault_in);
	seq_printf(m, "dax_write_prepare_ns %lld\ndax_write_range_ns %lld\n", prepare, range);
	seq_printf(m, "dax_write_range_wait_ns %lld\n", atomic64_read(&dax->write_range_wait_ns));
	/* Read apart, as writes go on: a difference may come out below zero by a little. */
	seq_printf(m, "dax_write_begin_ns %lld\ndax_write_begin_other_ns %lld\n", begin,
		   max_t(s64, 0, begin - prepare - range - atomic64_read(&dax->write_gate_ns) -
			 atomic64_read(&dax->write_fxfs_ns) - atomic64_read(&dax->write_map_ns) -
			 atomic64_read(&dax->write_wait_ns)));
	seq_printf(m, "dax_write_copy_ns %lld\ndax_write_end_ns %lld\n",
		   max_t(s64, 0, rw - begin - end), end);
	seq_printf(m, "dax_write_retry_ns %lld\ndax_write_tail_ns %lld\n", retry, tail);
	seq_printf(m, "dax_write_other_ns %lld\n",
		   max_t(s64, 0, atomic64_read(&dax->write_ns) - lock - setup - fault_in - rw -
			 retry - tail));
}

/*
 * fxdax: what dax_iomap_rw's `ret` says `iocb`'s write wrote, from `from`, less the bytes of the
 * iomap whose move was then dropped, or failed to commit (fxfs_dax_iomap_end), and of any after it:
 * those from `file->unwritten` on aren't the file's, and the position and `from` go back to it.
 * The range that iomap left behind the file's blocks is dropped, to be mapped again: a write
 * finding it behind took it for a second failure, and said EIO. If no bytes are left, why not.
 */
static ssize_t fxfs_dax_committed(struct kiocb *iocb, struct iov_iter *from, ssize_t ret)
{
	struct inode *inode = file_inode(iocb->ki_filp);
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	int err = file->unwritten_err, dropped;

	if (!err)
		return ret;
	file->unwritten_err = 0;
	if (ret > 0) {
		ret -= iocb->ki_pos - file->unwritten;
		iov_iter_revert(from, iocb->ki_pos - file->unwritten);
		iocb->ki_pos = file->unwritten;
	}
	dropped = fxfs_dax_drop_slice(inode, file->unwritten, true);
	if (ret)
		return ret;
	return dropped ?: err;
}

/*
 * fxdax: maps again at once the chunk whose slice `file`'s write left behind its blocks, which went
 * through another region (fxfs_dax_iomap_write): a slice of that region, which maps it already,
 * as the write would have, had the blocks moved first. Else the next write found it behind, and
 * made room in Fxfs again to map it, as for a chunk with no range. Failing, it is mapped as the
 * next use needs it.
 */
static void fxfs_dax_follow_slice(struct inode *inode, struct fxfs_dax_file *file)
{
	pgoff_t chunk = file->slice_left - 1;
	struct fxfs_dax_range *range;

	file->slice_left = 0;
	if (fxfs_dax_drop_slice(inode, (loff_t)chunk << FXFS_DAX_SHIFT, true))
		return;
	range = fxfs_dax_get_range(inode, chunk, 0, 0, false);
	if (!IS_ERR(range))
		fxfs_dax_put_range(fxfs_dax_of(inode), range);
}

/*
 * Writes through the window, copying with page faults disabled: in the gate, a fault on the
 * user's memory could wait for a seal that waits for the gate. The memory is faulted in first, and
 * again whenever a copy stops short. fxdax: and bytes are taken in hand, the rest of the chunk's,
 * and copied from there, a copy that can't stop short, once a copy stopped short with none copied
 * (-EFAULT): what faulted in may be gone again, or the write's own iomap unmaps it each time, the
 * source a page of the target. Then the write goes back to the user's memory.
 */
static ssize_t fxfs_dax_write_iter(struct kiocb *iocb, struct iov_iter *from)
{
	struct inode *inode = file_inode(iocb->ki_filp);
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_room room = fxfs_dax_room(dax);
	struct fxfs_dax_source hand = {};
	struct iov_iter *source = from;
	loff_t old_size, end, write_pos = 0;
	u64 seals = 0, began, lap;
	ssize_t ret, done = 0;
	unsigned int stale = 0, drops = 0, throttles = 0;
	bool made_room = false;
	int err, sealed;

	if (iocb->ki_flags & IOCB_NOWAIT)
		return -EAGAIN;
	began = lap = ktime_get_ns();
	inode_lock(inode);
	fxfs_dax_lap(&dax->write_lock_ns, &lap);
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
	end = write_pos + iov_iter_count(from);
	seals = fxfs_device_seals(fxfs_dax_of(inode)->dev);
	if (write_pos != old_size || file->append_next != write_pos || file->append_seals != seals)
		file->append_bytes = 0;
	/* The ranges the file's end moves into, mapped as large as they need. */
	if (iocb->ki_pos + iov_iter_count(from) > old_size) {
		WRITE_ONCE(file->growing, iocb->ki_pos + iov_iter_count(from));
		filemap_invalidate_lock(inode->i_mapping);
		ret = fxfs_dax_fit(inode, iocb->ki_pos + iov_iter_count(from));
		if (!ret && iocb->ki_pos > old_size)
			ret = fxfs_dax_zero_tail(inode, old_size, iocb->ki_pos);
		filemap_invalidate_unlock(inode->i_mapping);
		if (ret)
			goto out;
	}
	fxfs_dax_lap(&dax->write_setup_ns, &lap);
	while (iov_iter_count(source)) {
		bool unreadable = source == from &&
				  fault_in_iov_iter_readable(from, iov_iter_count(from)) ==
				  iov_iter_count(from);

		fxfs_dax_lap(&dax->write_fault_in_ns, &lap);
		if (unreadable) {
			ret = -EFAULT;
			break;
		}
		pagefault_disable();
		ret = dax_iomap_rw(iocb, source, &fxfs_dax_iomap_ops);
		pagefault_enable();
		ret = fxfs_dax_committed(iocb, source, ret);
		if (file->slice_left)
			fxfs_dax_follow_slice(inode, file);
		fxfs_dax_lap(&dax->write_rw_ns, &lap);
		sealed = ret < 0 ? fxfs_dax_make_memory(inode, &room, ret) : 0;
		if (ret > 0) {
			done += ret;
			drops = 0;
			room.seals = 0;
		} else if (sealed) {
			if (sealed < 0) {
				ret = sealed;
				break;
			}
			if (ret != -ENOMEM) {
				ret = fxfs_dax_drop_slice(inode, iocb->ki_pos, true);
				if (ret)
					break;
			}
			fxfs_dax_lap(&dax->write_retry_ns, &lap);
		} else if (ret == -ENOBUFS) {
			/*
			 * fxdax: copying on write, no room among the private pages: waited for,
			 * holding nothing of the file's but the inode's lock (doc-13's budget).
			 */
			ret = fxfs_dax_cow_throttle(dax, &throttles);
			if (ret)
				break;
		} else if (ret == -ENOSPC && dax->cow && !made_room) {
			/* fxdax: room made once, as the page cache's write_begin does. */
			fxfs_vfs_make_room(inode->i_sb);
			made_room = true;
		} else if (source == from && ret == -EFAULT) {
			ret = fxfs_dax_source_take(&hand, from, iocb->ki_pos);
			if (ret)
				break;
			source = &hand.iter;
		} else if (ret == -EXDEV || (ret == -ESTALE && !stale++)) {
			/*
			 * fxdax: a write that can't get on fails, and says so: it spun unkillable, with
			 * nothing in the log (TASK-140).
			 */
			if (fatal_signal_pending(current)) {
				ret = -EINTR;
				break;
			}
			if (++drops > FXFS_DAX_MAX_DROPS) {
				pr_err("fxfs: ino %lu: a write at %lld found no range to take it\n",
				       inode->i_ino, iocb->ki_pos);
				ret = -EIO;
				break;
			}
			if (drops > 1)
				WRITE_ONCE(file->no_slices, true);
			/*
			 * A slice the write can't use, or fxdax: a range mapping some of its blocks
			 * where they were (TASK-79.106), once: dropped, the chunk is mapped again.
			 */
			ret = fxfs_dax_drop_slice(inode, iocb->ki_pos, true);
			fxfs_dax_lap(&dax->write_retry_ns, &lap);
			if (ret)
				break;
		} else {
			if (ret == -ESTALE)
				ret = -EIO;
			break;
		}
		if (source != from && !iov_iter_count(source))
			source = from;
	}
	/* What it took and didn't write, it didn't write. */
	if (source != from)
		iov_iter_revert(from, iov_iter_count(source));
	kvfree(hand.kvec.iov_base);
	/*
	 * A write that stopped short leaves the file no longer than it wrote: Fxfs has the size its
	 * iomap asked for (fxfs_dax_map_write), past the inode's, which grows as bytes are written.
	 * One that wrote nothing leaves it as it was, wherever it began past the end.
	 */
	if (iocb->ki_pos < end && end > old_size) {
		/* fxdax: none written, past the end, it leaves the file as it was, not grown to `pos`. */
		err = fxfs_vfs_resize(inode, done ? max(old_size, iocb->ki_pos) : old_size, NULL, NULL);
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
	fxfs_dax_lap(&dax->write_tail_ns, &lap);
out:
	WRITE_ONCE(file->no_slices, false);
	WRITE_ONCE(file->growing, 0);
	inode_unlock(inode);
	began = ktime_get_ns() - began;
	atomic64_inc(&dax->writes);
	atomic64_add(began, &dax->write_ns);
	fxfs_hist_add(&dax->write_hist, began);
	if (ret > 0)
		ret = generic_write_sync(iocb, ret);
	return ret;
}

/* A fault of `order`, PAGE's or PMD's, writing if `write`. */
static vm_fault_t fxfs_dax_do_fault(struct vm_fault *vmf, unsigned int order, bool write)
{
	struct inode *inode = file_inode(vmf->vma->vm_file);
	struct super_block *sb = inode->i_sb;
	struct fxfs_dax_room room = fxfs_dax_room(fxfs_dax_of(inode));
	u64 began = ktime_get_ns();
	unsigned int stale = 0, waits = 0, drops = 0, throttles = 0;
	bool made_room = false;
	vm_fault_t ret;
	unsigned long pfn;
	int error, sealed;

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
		sealed = ret & VM_FAULT_ERROR ? fxfs_dax_make_memory(inode, &room, error) : 0;
		if (sealed > 0) {
			ret = error == -ENOMEM ? 0 : fxfs_dax_fault_drop_slice(vmf, ret);
			if (ret)
				break;
			continue;
		}
		if (sealed < 0)
			error = sealed;
		/*
		 * fxdax: copying on write, no room among the private pages: waited for, holding
		 * nothing of the file's, as balance_dirty_pages after a fault (doc-13's budget).
		 */
		if ((ret & VM_FAULT_ERROR) && error == -ENOBUFS) {
			error = fxfs_dax_cow_throttle(fxfs_dax_of(inode), &throttles);
			if (error) {
				ret = VM_FAULT_SIGBUS;
				break;
			}
			continue;
		}
		/*
		 * fxdax: no room in Fxfs for the page's writeback: once, room made by writing back
		 * what is dirty, as the page cache's page_mkwrite does (fxfs_vfs_make_room).
		 */
		if ((ret & VM_FAULT_ERROR) && error == -ENOSPC && fxfs_dax_of(inode)->cow &&
		    !made_room) {
			fxfs_vfs_make_room(sb);
			made_room = true;
			continue;
		}
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
			if (stale++)
				break;
			ret = fxfs_dax_fault_drop_slice(vmf, ret);
			if (ret)
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
			/*
			 * A slice can't take the store: it maps the chunk no more. fxdax: a fault that
			 * can't get on fails, and says so, as a write does: this is the one way round
			 * that waits for nothing, and only a seal would end it, if one came (TASK-140).
			 */
			if (++drops > FXFS_DAX_MAX_DROPS)
				break;
			ret = fxfs_dax_fault_drop_slice(vmf, ret);
			if (ret)
				break;
			continue;
		}
		/*
		 * No range was free, nor could the fault reclaim one, holding the lock it holds.
		 * fxdax: nor, for so long, could anything: it fails rather than wait for ever.
		 */
		if (++waits > FXFS_DAX_NO_ROOM_WAITS)
			break;
		fxfs_dax_make_room(fxfs_dax_of(inode), seen);
	}
	/* fxdax: a SIGBUS says nothing of why: the error here does (TASK-121). */
	if (ret & VM_FAULT_SIGBUS)
		pr_err_ratelimited("fxfs: a %s fault of order %u at page %lu of ino %lu failed: %d\n",
				   write ? "write" : "read", order, vmf->pgoff, inode->i_ino, error);
	if (write)
		sb_end_pagefault(sb);
	fxfs_hist_add(&fxfs_dax_of(inode)->fault_hist, ktime_get_ns() - began);
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

/*
 * fxdax: copying on write, the file's dirty pages written back (fxfs_dax_cow_write_back), whatever
 * `wbc` asks: the flusher's, every 30 s or so, and fsync's and sync's (doc-13).
 */
static int fxfs_dax_writepages(struct address_space *mapping, struct writeback_control *wbc)
{
	if (fxfs_dax_of(mapping->host)->cow)
		return fxfs_dax_cow_write_back(mapping->host);
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
	fxfs_dax_diagnosed();
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
	struct fxfs_dax_volume *volume;
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
	fxfs_dax_for_each_file(dax, volume, ino, file) {
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
	struct fxfs_dax_volume *volume;
	struct fxfs_dax_range *range;
	struct fxfs_dax_file *file;
	unsigned long ino, chunk;

	mutex_lock(&dax->files_lock);
	fxfs_dax_for_each_file(dax, volume, ino, file) {
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
 * fxdax: the region whose extent page `where` ((sequence << 20) | page, as the shadow has it) the
 * window maps at window page `page`, with a reference taken, which keeps it mapped there; or NULL,
 * if no region maps it so, as a range's own space or a region gone do.
 */
static struct fxfs_dax_region *fxfs_dax_hold_shadowed(struct fxfs_dax *dax, unsigned long page,
						      s64 where)
{
	struct fxfs_dax_region *region;
	u64 at = (u64)page << PAGE_SHIFT;
	unsigned long in_extent = where & ((1 << 20) - 1);

	spin_lock(&dax->regions_lock);
	region = fxfs_dax_find_region(dax, READ_ONCE(dax->shadow_branch), where >> 20);
	if (region && region->window_offset + ((u64)in_extent << PAGE_SHIFT) == at &&
	    in_extent < READ_ONCE(region->mapped))
		region->refs++;
	else
		region = NULL;
	spin_unlock(&dax->regions_lock);
	return region;
}

/*
 * fxdax: with fxfs.dax_verify_seals and the shadow, as a seal begins, no store in progress: each
 * window page of a region the guest mapped writably since the seal before, read through the
 * window, as the guest's stores reached it, is compared with its extent's page read from the
 * device, what the seal's commit is about to write, and where they differ logged (TASK-121).
 * fxdax: holding the region, which keeps it mapped: a restage's work unmapped a spare region the
 * shadow had as writable, and the read of it, which the host refused, stopped the VM (TASK-119.21,
 * GCE r236). A range's own space it can't hold, and leaves be.
 */
static void fxfs_dax_verify_writable(struct fxfs_dax *dax)
{
	u64 seals = fxfs_device_seals(dax->dev), then;
	struct fxfs_dax_region *region;
	unsigned long page;
	size_t at, differ;
	void *entry, *buf, *window;
	unsigned int checked = 0;
	s64 where;
	int id;

	buf = kmalloc(PAGE_SIZE, GFP_NOFS);
	if (!buf)
		return;
	xa_for_each(&dax->shadow, page, entry) {
		then = xa_to_value(entry) >> 1;
		if (!(xa_to_value(entry) & 1) || (then != seals - 1 && then != seals))
			continue;
		if (checked++ == 4096)
			break;
		where = fxfs_dax_read_shadowed(dax, page, buf);
		if (where < 0)
			continue;
		region = fxfs_dax_hold_shadowed(dax, page, where);
		if (!region)
			continue;
		id = dax_read_lock();
		window = fxfs_dax_window_page(dax, (u64)page << PAGE_SHIFT);
		differ = window ? fxfs_dax_differ(window, buf, 0, 0, &at, PAGE_SIZE) : 0;
		/*
		 * fxdax: Fxfs's device WRITE of what a move keeps around a write is in no gate, and may
		 * come between the two reads: the device's, read again, has it by now, if it wasn't this.
		 */
		if (differ) {
			dax_read_unlock(id);
			fxfs_dax_read_shadowed(dax, page, buf);
			id = dax_read_lock();
			window = fxfs_dax_window_page(dax, (u64)page << PAGE_SHIFT);
			differ = window ? fxfs_dax_differ(window, buf, 0, 0, &at, PAGE_SIZE) : 0;
		}
		if (differ) {
			pr_err("fxfs: window page %lx, writable at seals %llu, mapping extent %llu page %llu: %zu bytes differ from the device's, the first at %zu: %#x through the window, %#x the device's\n",
			       page, then, where >> 20, where & ((1 << 20) - 1), differ, at,
			       ((u8 *)window)[at], ((u8 *)buf)[at]);
			fxfs_dax_diagnosed();
		}
		dax_read_unlock(id);
		fxfs_dax_put_region(dax, region);
	}
	kfree(buf);
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
	/*
	 * fxdax: copying on write, a seal changes nothing in the window (doc-13): its writable PTEs
	 * are stores the host copies, which the seal leaves be.
	 */
	if (dax->cow)
		return;
	fxfs_dax_check_ptes(dax);
	spin_lock(&dax->regions_lock);
	list_for_each_entry_safe(region, next_region, &dax->writable_regions, writable_list) {
		WRITE_ONCE(region->writable, false);
		list_del_init(&region->writable_list);
		fxfs_dax_give_window(dax, region->window_offset + ((u64)region->mapped << PAGE_SHIFT),
				     region->pages - region->mapped);
		/* fxdax: as fxfs_dax_grow_ahead reads it, outside the gate (KCSAN, GCE r255). */
		WRITE_ONCE(region->pages, region->mapped);
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
	}
	mutex_unlock(&dax->writable_lock);
	if (READ_ONCE(fxfs_dax_check_seals))
		fxfs_dax_check_sealed(dax);
	if (READ_ONCE(fxfs_dax_verify_seals) && READ_ONCE(fxfs_dax_shadowed))
		fxfs_dax_verify_writable(dax);
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
	/* fxdax: copying on write, page-cache inodes evicted before it may be read in to DAX. */
	if (dax->cow && atomic_long_read(&dax->unsealed))
		fxfs_dax_cow_kick(dax, false);
}

/*
 * fxdax: replaces the staging extent sealed, if writes took blocks of it, and maps the new one's
 * region writably, as the first write to it would: so that the next small files, slices of it,
 * wait for no MAP. It keeps the region until the next replaces it. The new extent is open only while no seal comes between
 * it and its mapping: if one does, it isn't mapped, and the allocator takes another.
 */
static void fxfs_dax_restage_work(struct work_struct *work)
{
	struct fxfs_dax *dax = container_of(work, struct fxfs_dax, restage);
	struct fxfs_dax_region *region, *old;
	struct fxfs_dax_extent e;
	u64 seals = fxfs_device_seals(dax->dev);

	if (!fxfs_nofs(fxfs_dax_restage(dax->filesystem, &e)))
		goto out;
	region = fxfs_dax_get_region(dax, &e, false);
	if (IS_ERR(region))
		goto out;
	if (fxfs_device_store_begin(dax->dev) == seals) {
		fxfs_dax_region_open(dax, region);
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
 * fxdax: the window's regions by what they map, writably, or a sealed extent's, with the references
 * held to them; and the slices that hold them.
 */
static void fxfs_dax_show_regions(struct fxfs_dax *dax, struct seq_file *m)
{
	static const char *const kinds[] = { "writable", "read" };
	unsigned long count[2] = {}, pages[2] = {}, refs[2] = {}, slices = 0;
	struct fxfs_dax_region *region;
	struct fxfs_dax_volume *volume;
	struct fxfs_dax_range *range;
	struct fxfs_dax_file *file;
	unsigned long ino, chunk;
	unsigned int bucket;
	int kind;

	spin_lock(&dax->regions_lock);
	hash_for_each(dax->regions, bucket, region, node) {
		kind = region->writable ? 0 : 1;
		count[kind]++;
		pages[kind] += region->pages;
		refs[kind] += region->refs;
	}
	spin_unlock(&dax->regions_lock);
	mutex_lock(&dax->files_lock);
	fxfs_dax_for_each_file(dax, volume, ino, file) {
		/*
		 * Ranges freed meanwhile are freed after a grace period (fxfs_dax_free_range), but not
		 * their regions: only the ranges are read.
		 */
		rcu_read_lock();
		xa_for_each(&file->ranges, chunk, range)
			slices += !!READ_ONCE(range->region);
		rcu_read_unlock();
	}
	mutex_unlock(&dax->files_lock);
	for (kind = 0; kind < ARRAY_SIZE(kinds); kind++)
		seq_printf(m, "window_regions_%s %lu\nwindow_regions_%s_bytes %lu\nwindow_regions_%s_refs %lu\n",
			   kinds[kind], count[kind], kinds[kind], pages[kind] << PAGE_SHIFT,
			   kinds[kind], refs[kind]);
	seq_printf(m, "window_slices %lu\n", slices);
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
	seq_printf(m, "window_regions %ld\n", atomic_long_read(&dax->region_count));
	fxfs_dax_show_regions(dax, m);
	seq_printf(m, "dax_map_writes %lld\n", atomic64_read(&dax->map_writes));
	seq_printf(m, "dax_writes_through %lld\ndax_catch_ups %lld\ndax_caught_up_pages %lld\n",
		   atomic64_read(&dax->writes_through), atomic64_read(&dax->catch_ups),
		   atomic64_read(&dax->caught_up));
	seq_printf(m, "window_lost_bytes %lld\n", atomic64_read(&dax->lost_bytes));
	seq_printf(m, "dax_huge_maps %lld\n", atomic64_read(&dax->huge_maps));
	seq_printf(m, "dax_extent_alloc_bytes %lld\n", atomic64_read(&dax->extent_alloc_bytes));
	seq_printf(m, "dax_write_waits %lld\ndax_write_wait_ns %lld\n",
		   atomic64_read(&dax->write_waits), atomic64_read(&dax->write_wait_ns));
	seq_printf(m, "dax_moved_unsealed %lld\n", atomic64_read(&dax->moved_unsealed));
	seq_printf(m, "dax_writes_dropped %lld\n", atomic64_read(&dax->writes_dropped));
	seq_printf(m, "dax_maps_refused %lld\n", atomic64_read(&dax->maps_refused));
	seq_printf(m, "dax_memory_seals %lld\n", atomic64_read(&dax->memory_seals));
	/* fxdax: copying on write (doc-13, TASK-164.2): 0s without it. */
	seq_printf(m, "dax_cow %d\ndax_cow_private_max %ld\n", dax->cow, dax->private_max);
	seq_printf(m, "dax_cow_private_pages %ld\ndax_cow_dirty_pages %ld\n",
		   atomic_long_read(&dax->private), atomic_long_read(&dax->dirty));
	seq_printf(m, "dax_cow_writebacks %lld\ndax_cow_written_pages %lld\n",
		   atomic64_read(&dax->cow_writebacks), atomic64_read(&dax->cow_written));
	seq_printf(m, "dax_cow_rebound_pages %lld\ndax_cow_throttled %lld\n",
		   atomic64_read(&dax->cow_rebound), atomic64_read(&dax->cow_throttled));
	seq_printf(m, "dax_cow_seals %lld\ndax_cow_privates %lld\ndax_cow_open_maps %lld\n",
		   atomic64_read(&dax->cow_seals), atomic64_read(&dax->cow_privates),
		   atomic64_read(&dax->cow_open_maps));
	seq_printf(m, "dax_cow_owns %lld\n", atomic64_read(&dax->cow_owns));
	seq_printf(m, "dax_cow_unsealed_inodes %ld\n", atomic_long_read(&dax->unsealed));
	seq_printf(m, "dax_writes %lld\ndax_writes_ns %lld\n", atomic64_read(&dax->writes),
		   atomic64_read(&dax->write_ns));
	seq_printf(m, "dax_write_lock_ns %lld\ndax_write_gate_ns %lld\n",
		   atomic64_read(&dax->write_lock_ns), atomic64_read(&dax->write_gate_ns));
	seq_printf(m, "dax_write_fxfs_ns %lld\ndax_write_map_ns %lld\n",
		   atomic64_read(&dax->write_fxfs_ns), atomic64_read(&dax->write_map_ns));
	fxfs_dax_show_write_times(m, dax);
	fxfs_hist_show(m, "dax_write", &dax->write_hist);
	fxfs_hist_show(m, "dax_faults", &dax->fault_hist);
	for (why = 0; why < FXFS_DAX_WHYS; why++)
		seq_printf(m, "window_maps_%s %lld\nwindow_map_bytes_%s %lld\n", fxfs_dax_whys[why],
			   atomic64_read(&dax->maps[why]), fxfs_dax_whys[why],
			   atomic64_read(&dax->map_bytes[why]));
}

/*
 * Destroys `pool`, whatever of it is taken: gen_pool_destroy stops the kernel if anything is, and
 * the window's pages an UNMAP failed for are kept taken for good (fxfs_dax_give_unmapped,
 * TASK-150.12).
 */
static void fxfs_dax_destroy_pool(struct gen_pool *pool)
{
	struct gen_pool_chunk *chunk;

	list_for_each_entry(chunk, &pool->chunks, next_chunk)
		bitmap_zero(chunk->bits,
			    (chunk->end_addr - chunk->start_addr + 1) >> pool->min_alloc_order);
	gen_pool_destroy(pool);
}

/* Destroys the window's pools: the ranges', and the regions', if apart. */
static void fxfs_dax_destroy_pools(struct fxfs_dax *dax)
{
	if (dax->region_pool && dax->region_pool != dax->pool)
		fxfs_dax_destroy_pool(dax->region_pool);
	if (dax->pool)
		fxfs_dax_destroy_pool(dax->pool);
}

struct fxfs_dax *fxfs_dax_create(struct fxfs_filesystem *filesystem)
{
	struct fxfs_device *dev = fxfs_dax_device(filesystem);
	struct dax_device *dax_dev;
	struct fxfs_dax *dax;
	u64 window_len;

	dax_dev = fxfs_device_dax(dev, &window_len);
	if (!dax_dev || window_len < FXFS_DAX_SIZE)
		return NULL;
	/* fxdax: copying on write, a few iomaps' worth of private pages at least (doc-13). */
	if (fxfs_device_dax_cow(dev) && fxfs_device_private_max(dev) < 8) {
		pr_warn("fxfs: the DAX window copies on write, but holds %llu private pages: no DAX\n",
			fxfs_device_private_max(dev));
		return NULL;
	}
	dax = kzalloc(sizeof(*dax), GFP_KERNEL);
	if (!dax)
		return ERR_PTR(-ENOMEM);
	dax->pool = gen_pool_create(PAGE_SHIFT, -1);
	/*
	 * fxdax: half a window of four chunks or more, in chunks, for regions (TASK-91.10): a quarter
	 * filled as pgbench -i -s 50 loaded its tables, whose writes then mapped their pieces in
	 * ranges' space, the load half as slow again.
	 */
	dax->window_len = window_len;
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
	dax->filesystem = filesystem;
	dax->dev = dev;
	WRITE_ONCE(fxfs_dax_stray_dax, dax);
	/* fxdax: the trace, if asked for, of the filesystem mounted last (TASK-121). */
	if (fxfs_dax_tracing) {
		void *kaddr;
		int id = dax_read_lock();

		if (dax_direct_access(dax_dev, 0, 1, DAX_ACCESS, &kaddr, &fxfs_dax_trace_base) < 1)
			fxfs_dax_trace_base = 0;
		dax_read_unlock(id);
		WRITE_ONCE(fxfs_dax_trace_dev, dev);
		if (!fxfs_dax_trace_events)
			WRITE_ONCE(fxfs_dax_trace_events,
				   vzalloc(sizeof(*fxfs_dax_trace_events) * FXFS_DAX_TRACE_EVENTS));
	}
	dax->dax_dev = dax_dev;
	INIT_LIST_HEAD(&dax->volumes);
	xa_init(&dax->shadow);
	xa_init(&dax->shadow_extent);
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
	/*
	 * fxdax: copying on write (doc-13), as the driver took DAX_COW for fxfs.dax_cow: no staging
	 * extent's region, nothing written through the window; the budget's worker; and at least a
	 * few iomaps' worth of private pages, else no DAX.
	 */
	dax->cow = fxfs_device_dax_cow(dev);
	dax->restaging = !dax->cow;
	INIT_WORK(&dax->cow_work, fxfs_dax_cow_work);
	init_waitqueue_head(&dax->cow_wait);
	if (dax->cow) {
		dax->private_max = min_t(u64, fxfs_device_private_max(dev), LONG_MAX);
		dax->cow_pages = clamp_t(unsigned long, dax->private_max / 8, 1, FXFS_DAX_COW_PAGES);
	}
	fxfs_device_set_sealer(dev, fxfs_dax_seal, fxfs_dax_sealed, dax);
	return dax;
}

struct fxfs_dax_volume *fxfs_dax_add_volume(struct fxfs_dax *dax, struct fxfs *fs)
{
	struct fxfs_dax_volume *volume = kzalloc(sizeof(*volume), GFP_KERNEL);

	if (!volume)
		return ERR_PTR(-ENOMEM);
	volume->fs = fs;
	xa_init(&volume->files);
	xa_init(&volume->unsealed);
	mutex_lock(&dax->files_lock);
	list_add(&volume->list, &dax->volumes);
	mutex_unlock(&dax->files_lock);
	return volume;
}

void fxfs_dax_remove_volume(struct fxfs_dax *dax, struct fxfs_dax_volume *volume)
{
	struct fxfs_dax_range *range;
	struct fxfs_dax_file *file;
	unsigned long ino, chunk;
	void *entry;

	/* fxdax: copying on write, the budget's worker, which uses the volume's files, waits. */
	fxfs_dax_stop(dax);
	mutex_lock(&dax->writable_lock);
	mutex_lock(&dax->files_lock);
	xa_for_each(&volume->files, ino, file)
		list_del_init(&file->writable);
	mutex_unlock(&dax->files_lock);
	mutex_unlock(&dax->writable_lock);

	/* Reclaim may hold one of its files still: it finds it has no volume. */
	mutex_lock(&dax->files_lock);
	list_del(&volume->list);
	xa_for_each(&volume->files, ino, file) {
		WARN_ON_ONCE(file->inode);
		file->volume = NULL;
	}
	mutex_unlock(&dax->files_lock);

	/* Its inodes evicted, nothing uses its ranges. */
	xa_for_each(&volume->files, ino, file) {
		xa_erase(&volume->files, ino);
		down_write(&file->sem);
		xa_for_each(&file->ranges, chunk, range) {
			if (WARN_ON_ONCE(atomic_read(&range->users)))
				continue;
			fxfs_dax_unmap_range(dax, range);
		}
		up_write(&file->sem);
		fxfs_dax_put_file(dax, file);
	}
	xa_destroy(&volume->files);
	xa_for_each(&volume->unsealed, ino, entry)
		atomic_long_dec(&dax->unsealed);
	xa_destroy(&volume->unsealed);
	kfree(volume);
	fxfs_dax_start(dax);
}

/*
 * Waits for the regions growing in the background to be as far ahead of the writes so far as
 * they grow, and for the spare to be mapped (fxfs_dax_restage_work): before a sync seals them. A
 * few rounds at most, as writes may go on meanwhile.
 */
int fxfs_dax_settle(struct fxfs_dax *dax)
{
	struct fxfs_dax_file **files;
	size_t nr, i;
	int round, err = 0;

	for (round = 0; round < 8 && atomic_read(&dax->growing); round++)
		flush_workqueue(dax->grow_wq);
	if (!dax->cow)
		return 0;
	/*
	 * fxdax: copying on write, every file's dirty pages written back, which a write(2)'s leave
	 * the mapping untagged for, so that sync's writepages miss them (doc-13); and those of a
	 * writeback under way committed, which fxfs_dax_cow_write_back waits for: a file whose pages
	 * the budget's writeback had swapped out had none dirty, and was sealed without them. Each
	 * holding dirty or private pages, as a writeback's are until a rebind after its commit.
	 */
	nr = fxfs_dax_cow_files(dax, &files);
	for (i = 0; i < nr; i++) {
		struct inode *inode = fxfs_dax_cow_inode(dax, files[i]);

		if (inode) {
			int wrote = fxfs_dax_cow_write_back(inode);

			err = err ?: wrote;
			iput(inode);
		}
		fxfs_dax_put_file(dax, files[i]);
	}
	kvfree(files);
	return err;
}

/*
 * fxdax: copying on write, stops the budget's worker, and waits for it: before a volume goes, or the
 * filesystem unmounts, or freezes. A write it would have made room for waits meanwhile.
 */
void fxfs_dax_stop(struct fxfs_dax *dax)
{
	WRITE_ONCE(dax->cow_stopping, true);
	cancel_work_sync(&dax->cow_work);
}

/* fxdax: starts the budget's worker again, unless the filesystem is frozen, and has it run. */
void fxfs_dax_start(struct fxfs_dax *dax)
{
	if (READ_ONCE(dax->frozen))
		return;
	WRITE_ONCE(dax->cow_stopping, false);
	if (dax->cow)
		fxfs_dax_cow_kick(dax, false);
}

/*
 * fxdax: copying on write, the filesystem frozen, durably, its writers stopped and their pages
 * written back as the VFS synced it: every private page is rebound, mapping its extent's page,
 * sealed now, so that the snapshot's window holds extents and zeros alone, as the device has it
 * (doc-13). Those it can't, warned of: the device refuses the snapshot.
 */
void fxfs_dax_frozen(struct fxfs_dax *dax)
{
	WRITE_ONCE(dax->frozen, true);
	if (!dax->cow)
		return;
	fxfs_dax_cow_relieve(dax, 0, false);
	if (atomic_long_read(&dax->private))
		pr_warn("fxfs: frozen with %ld private pages, %ld dirty: no snapshot\n",
			atomic_long_read(&dax->private), atomic_long_read(&dax->dirty));
}

/*
 * Waits for the regions growing in the background, once nothing writes any more: before the
 * filesystem unmounts, or freezes, while its device takes requests. The spare region, whose
 * extent is open, goes, so that the seal after leaves nothing open.
 */
void fxfs_dax_quiesce(struct fxfs_dax *dax)
{
	struct fxfs_dax_region *spare;

	/* fxdax: nor rounds of the budget's worker, which may requeue itself. */
	fxfs_dax_stop(dax);
	/* No more spares: the filesystem's last seals come as it unmounts, or freezes. */
	WRITE_ONCE(dax->restaging, false);
	if (cancel_work_sync(&dax->restage))
		atomic_dec(&dax->growing);
	drain_workqueue(dax->grow_wq);
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
	WRITE_ONCE(dax->restaging, !dax->cow);
	WRITE_ONCE(dax->frozen, false);
	fxfs_dax_start(dax);
}

/*
 * Frees `dax` once the filesystem is unmounted, its inodes evicted: the window's ranges are
 * unmapped as the device goes.
 */
void fxfs_dax_destroy(struct fxfs_dax *dax)
{
	struct fxfs_dax_volume *volume, *next_volume;
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
	fxfs_dax_stop(dax);
	/* fxdax: no longer traced: the device goes with the mount. */
	if (READ_ONCE(fxfs_dax_trace_dev) == dax->dev)
		WRITE_ONCE(fxfs_dax_trace_dev, NULL);
	if (READ_ONCE(fxfs_dax_stray_dax) == dax)
		WRITE_ONCE(fxfs_dax_stray_dax, NULL);
	list_for_each_entry_safe(range, next, &dax->used, list) {
		xa_erase(&range->file->ranges, range->chunk);
		list_del(&range->list);
		/* Its regions go below, and its space with the pools. */
		fxfs_dax_free_range_memory(range);
	}
	hash_for_each_safe(dax->regions, bucket, tmp, region, node) {
		hash_del(&region->node);
		kfree(region);
	}
	list_for_each_entry_safe(volume, next_volume, &dax->volumes, list) {
		xa_for_each(&volume->files, ino, file) {
			xa_erase(&volume->files, ino);
			fxfs_dax_put_file(dax, file);
		}
		xa_destroy(&volume->files);
		xa_destroy(&volume->unsealed);
		kfree(volume);
	}
	xa_destroy(&dax->shadow);
	xa_destroy(&dax->shadow_extent);
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
	struct fxfs_dax_volume *volume = fxfs_dax_volume_of(inode);
	struct fxfs_dax_file *file;

	mutex_lock(&dax->files_lock);
	file = xa_load(&volume->files, inode->i_ino);
	if (!file) {
		file = kzalloc(sizeof(*file), GFP_NOFS);
		if (!file)
			goto out;
		file->volume = volume;
		file->ino = inode->i_ino;
		refcount_set(&file->refs, 1);
		init_rwsem(&file->sem);
		mutex_init(&file->wb_lock);
		xa_init(&file->ranges);
		INIT_LIST_HEAD(&file->writable);
		if (xa_err(xa_store(&volume->files, inode->i_ino, file, GFP_NOFS))) {
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

/*
 * fxdax: copying on write, whether the page-cache inode `ino`, of `volume`, has writes no barrier
 * has sealed yet (`unsealed`): read in now, the window would find their extents open, which it
 * maps none of (doc-13).
 */
static bool fxfs_dax_cow_unsealed(struct fxfs_dax *dax, struct fxfs_dax_volume *volume, u64 ino)
{
	void *entry = xa_load(&volume->unsealed, ino);

	if (!entry)
		return false;
	if (fxfs_device_seals(dax->dev) < xa_to_value(entry))
		return true;
	if (xa_cmpxchg(&volume->unsealed, ino, entry, NULL, GFP_NOFS) == entry)
		atomic_long_dec(&dax->unsealed);
	return false;
}

void fxfs_dax_written(struct inode *inode)
{
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_volume *volume;
	void *was;

	if (!dax || !dax->cow || IS_DAX(inode))
		return;
	volume = fxfs_dax_volume_of(inode);
	was = xa_store(&volume->unsealed, inode->i_ino, xa_mk_value(fxfs_dax_seal_target(dax)),
		       GFP_NOFS);
	/* Without the memory, it is read in to DAX, whose mapping seals what it finds open. */
	if (!xa_is_err(was) && !was)
		atomic_long_inc(&dax->unsealed);
}

bool fxfs_dax_set_inode(struct inode *inode, bool created)
{
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_file *file = NULL;

	/*
	 * fxdax: copying on write, an inode created in this mount is a page-cache inode while it is in
	 * memory, as one whose writes no barrier has sealed yet, read in again: S_DAX is set as an
	 * inode is read in, never on a live one, as ext4 and XFS have it (doc-13, TASK-164.3).
	 */
	if (dax->cow && (created || fxfs_dax_cow_unsealed(dax, fxfs_dax_volume_of(inode),
							  inode->i_ino)))
		return false;
	if (!READ_ONCE(fxfs_dax_fail_attach))
		file = fxfs_dax_attach(dax, inode);
	/* Without its file, it stays in the page cache. */
	if (!file)
		return false;
	fxfs_vfs_inode(inode)->dax.file = file;
	inode->i_flags |= S_DAX;
	inode->i_fop = &fxfs_dax_file_operations;
	inode->i_mapping->a_ops = &fxfs_dax_aops;
	return true;
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
	int err;

	if (!dax || !IS_DAX(inode) || !file)
		return;
	/* fxdax: copying on write, its dirty pages written back, which its ranges outlive. */
	if (dax->cow && inode->i_nlink) {
		err = fxfs_dax_cow_write_back(inode);
		if (err)
			pr_warn("fxfs: writing back ino %lu as it is evicted: %d\n", inode->i_ino, err);
	}
	if (!inode->i_nlink) {
		down_write(&file->sem);
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

int fxfs_dax_changed(struct fxfs_dax *dax, struct fxfs_dax_volume *volume, struct inode *inode,
		     u64 ino, loff_t start, loff_t end)
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
		file = xa_load(&volume->files, ino);
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
			err = fxfs_dax_changed(dax, volume, inode, ino, start, end);
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
 * fuse_dax_break_layouts: with the invalidate_lock held for writing. From the start of its chunk:
 * a PMD's entry goes whole, and a mapping may have pages of it before `start` mapped, which
 * dax_folio_put then finds in use.
 */
static int fxfs_dax_break_layouts(struct inode *inode, loff_t start)
{
	lockdep_assert_held_write(&inode->i_mapping->invalidate_lock);

	return dax_break_layout(inode, round_down(start, FXFS_DAX_SIZE), LLONG_MAX,
				fxfs_dax_wait_page);
}

/*
 * Zeroes the `len` bytes of `inode` at `pos`, in one page, through the window: dropping a slice
 * that can't take them where it is, to be mapped again. With the invalidate_lock held for writing.
 */
static int fxfs_dax_zero_part(struct inode *inode, loff_t pos, loff_t len)
{
	struct fxfs_dax_room room = fxfs_dax_room(fxfs_dax_of(inode));
	unsigned int stale = 0, throttles = 0;
	bool zeroed;
	int sealed, err;

	for (;;) {
		err = dax_zero_range(inode, pos, len, &zeroed, &fxfs_dax_iomap_ops);
		/* fxdax: copying on write, no room among the private pages: the worker makes some. */
		if (err == -ENOBUFS) {
			err = fxfs_dax_cow_throttle(fxfs_dax_of(inode), &throttles);
			if (err)
				return err;
			continue;
		}
		sealed = fxfs_dax_make_memory(inode, &room, err);
		if (sealed < 0)
			return sealed;
		if (sealed && err == -ENOMEM)
			continue;
		/* fxdax: a range behind Fxfs's blocks is dropped as a slice is, once (TASK-150.13). */
		if (!sealed && err == -ESTALE && stale++)
			return -EIO;
		if (err != -EXDEV && err != -ESTALE)
			return err;
		err = fxfs_dax_drop(inode, pos >> FXFS_DAX_SHIFT, pos >> FXFS_DAX_SHIFT, -1);
		if (err)
			return err;
	}
}

/*
 * fxdax: as a file `old` bytes long grows to `size`, zeroes what its last page holds past `old` and
 * short of `size`: a mapping's stores there aren't the file's, and growing would have reads find
 * them (TASK-79.53). Here, not left to Fxfs, whose size may be past `old` as a failed write's is, and
 * which then zeroes from there. With the invalidate_lock held for writing.
 */
static int fxfs_dax_zero_tail(struct inode *inode, loff_t old, loff_t size)
{
	if (!offset_in_page(old))
		return 0;
	return fxfs_dax_zero_part(inode, old, min_t(loff_t, size, round_up(old, PAGE_SIZE)) - old);
}

/*
 * Readies the last block of `inode`, `old` bytes long, for Fxfs growing the file to `size` to copy
 * it, zeroed past `old`, through the device: what the window holds past the end zeroed, its stores
 * sealed, and its mapping dropped, which the copy replaces, or the window could hide the copy until
 * eviction. Fxfs's growths zero the block so: a truncate's, and a clone's (TASK-153.4). With
 * the invalidate_lock held for writing.
 */
static int fxfs_dax_grow_begin(struct inode *inode, loff_t old, loff_t size)
{
	int err;

	if (size <= old || PAGE_ALIGNED(old))
		return 0;
	err = fxfs_dax_zero_tail(inode, old, size);
	if (!err)
		err = fxfs_nofs(fxfs_fsync(fxfs_vfs_fs(inode->i_sb)));
	if (!err)
		err = fxfs_dax_break_layouts(inode, round_down(old, PAGE_SIZE));
	if (!err)
		err = fxfs_dax_drop(inode, old >> FXFS_DAX_SHIFT, old >> FXFS_DAX_SHIFT, -1);
	return err;
}

/*
 * Their DAX entries go, once their pages are unused, and the window's ranges of them, to be mapped
 * again as they are after the reflink: a source's blocks are shared from then on, which its ranges
 * may map writably, to be written in place (fxfs_dax_map_extents), as they can't be any more; and a
 * destination's are other blocks. And the destination's ranges too small for its new size, and its
 * last block, which Fxfs zeroes past the end as it grows (fxfs_dax_grow_begin).
 */
int fxfs_dax_remap(struct inode *inode, loff_t pos, loff_t len, loff_t size)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	int err;

	if (!IS_DAX(inode) || !fxfs_dax_file(inode) || !len)
		return 0;
	/*
	 * fxdax: copying on write, Fxfs's size is as writeback leaves it: the ranges dropped below
	 * are written back first (fxfs_dax_drop_pinned), as XFS writes back both files' ranges.
	 */
	down_write(&file->sem);
	err = fxfs_dax_of(inode)->cow ? 0 : fxfs_dax_write_size_locked(inode);
	up_write(&file->sem);
	if (!err)
		err = fxfs_dax_grow_begin(inode, i_size_read(inode), size);
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
	bool cow = fxfs_dax_of(inode)->cow;
	int err;

	filemap_invalidate_lock(inode->i_mapping);
	/*
	 * An ordinary grow zeroes from Fxfs's end: first record data appended since it. fxdax:
	 * copying on write, Fxfs's end is as writeback leaves it, which writes what is dirty first,
	 * so that it is the inode's (doc-13).
	 */
	if (cow) {
		err = fxfs_dax_cow_write_back(inode);
	} else {
		down_write(&file->sem);
		err = fxfs_dax_write_size_locked(inode);
		up_write(&file->sem);
	}
	if (err) {
		filemap_invalidate_unlock(inode->i_mapping);
		return err;
	}
	if (size >= i_size_read(inode)) {
		/* An ordinary grow copies and zeroes the last partial block through the device. */
		err = fxfs_dax_grow_begin(inode, i_size_read(inode), size);
		/* The ranges the file's end moves into, mapped as large as they need. */
		if (!err)
			err = fxfs_dax_fit(inode, size);
	} else {
		err = fxfs_dax_break_layouts(inode, round_up(size, PAGE_SIZE));
		/* What the last page holds past the end reads as zeros if the file grows again. */
		if (!err && offset_in_page(size))
			err = fxfs_dax_zero_part(inode, size, PAGE_SIZE - offset_in_page(size));
	}
	if (err) {
		filemap_invalidate_unlock(inode->i_mapping);
		return err;
	}
	/*
	 * fxdax: copying on write, no writeback until fxfs_dax_resize_end: one that read the size
	 * before the change wrote the pages past the new end after Fxfs cut them, and grew the file
	 * again. Taken last, as zeroing above may wait for the budget's worker, which writes back.
	 */
	if (cow)
		mutex_lock(&file->wb_lock);
	return 0;
}

void fxfs_dax_resize_end(struct inode *inode, loff_t size)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	unsigned long first = DIV_ROUND_UP(size, PAGE_SIZE), page, chunk;
	struct fxfs_dax_range *range;
	int err;

	down_write(&file->sem);
	xa_for_each_start(&file->ranges, chunk, range, size >> FXFS_DAX_SHIFT) {
		loff_t start = fxfs_dax_chunk_start(range);

		if (start >= round_up(size, PAGE_SIZE)) {
			fxfs_dax_unmap_range(dax, range);
			continue;
		}
		page = first - (start >> PAGE_SHIFT);
		if (page >= range->pages)
			continue;
		bitmap_clear(range->lagging, page, range->pages - page);
		if (find_next_bit(range->mapped, range->pages, page) == range->pages)
			continue;
		/*
		 * fxdax: copying on write, the pages past the end map zeros, which every page of a
		 * range maps (fxfs_dax_cow_map), the MAP freeing their private pages, their dirty
		 * pages' room released; unmapped if it fails, as a write maps them again.
		 */
		if (dax->cow && !range->region) {
			err = fxfs_device_map_zeros(dax->dev, fxfs_dax_page_offset(range, page),
						    (u64)(range->pages - page) << PAGE_SHIFT);
			if (!err) {
				fxfs_dax_cow_forget(dax, range, page, range->pages - page, true);
				bitmap_set(range->zeros, page, range->pages - page);
				continue;
			}
			pr_warn("fxfs: mapping zeros past ino %lu's end at %llx of the DAX window failed: %d\n",
				inode->i_ino, fxfs_dax_page_offset(range, page), err);
		}
		/* A slice's pages past the end stay its region's: the slice no longer maps them. */
		err = range->region ? 0 :
		      fxfs_dax_device_unmap(dax, range->window_offset + ((u64)page << PAGE_SHIFT),
					(u64)(range->pages - page) << PAGE_SHIFT);
		if (err)
			pr_warn("fxfs: unmapping the DAX window at %llx failed: %d\n",
				range->window_offset, err);
		/* fxdax: copying on write, the UNMAP freed their private pages, unless it failed. */
		fxfs_dax_cow_forget(dax, range, page, range->pages - page, !err);
		bitmap_clear(range->mapped, page, range->pages - page);
		fxfs_dax_clear_writable(range, page, range->pages - page);
	}
	up_write(&file->sem);
	if (dax->cow)
		mutex_unlock(&file->wb_lock);
	filemap_invalidate_unlock(inode->i_mapping);
}
