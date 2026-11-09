// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * Files' data mapped from the extent-store device's DAX window (backlog doc-2), as virtio-fs's DAX
 * maps a host's files (fs/fuse/dax.c), instead of the page cache: reads, writes and mappings of
 * files reach the extents the host shares, through the window.
 *
 * The window is split into ranges, each mapping a chunk of FXFS_DAX_SIZE bytes of one file. A
 * range is as large as its chunk of the file, in pages: a small file's takes what it needs of the
 * window, rounded up to a power of two, so that a file growing a page at a time maps its data again
 * only as often as it doubles.
 *
 * The window copies on write (backlog doc-13, TASK-164). It maps sealed extents, and zeros for
 * holes and pages past a file's end, read-only, nothing open: every page of a range has a window
 * page. A store to a page (a fault's, a write(2)'s copy) makes it private on the host, the VM's
 * own, holding what it held, at the same address. A page's state is dax.c's alone, its range's,
 * which only pages.c's transitions change (TASK-160.4): the iomap that dirties a page has the host
 * make it private first (PRIVATE), so that a page is dirty only if private, whenever its store
 * lands, and joins its file's dirty set, with room reserved in Fxfs for its writeback, counted
 * against the device's private_max (fxfs_dax_dirty). Writeback (fsync, sync, writepages, the
 * budget) swaps the set out, the writeback set, write-protects what faults made writable by the
 * page tables, and has Fxfs write the pages' bytes from the window to new extents, allocated then,
 * committing after them, as the page cache's writeback does (fxfs_dax_write_back). Once the barrier
 * after it has sealed those extents, a private page in neither set may be mapped back to its
 * extent's page, the same bytes at the same address (fxfs_dax_rebind), under its file's semaphore
 * held for writing, which sees no store to it under way. An UNMAP, or a truncate's MAP of zeros,
 * ends what it covers (fxfs_dax_dropped). The kernel's DAX entries' dirty tags and locks decide
 * none of it: they are cleaned for the kernel's sake alone (fxfs_dax_clean_entries). Nothing moves,
 * so nothing seals but what Fxfs commits; files created in a mount are page-cache inodes until read
 * in again (fxfs_dax_set_inode).
 *
 * A chunk whose data is one piece of a sealed extent of at most a chunk's size, as small files'
 * are, sharing extents (the allocator's staging extents), is a slice of that extent's region
 * instead: the window's mapping of the whole extent, from its start, read-only, which all the files
 * it holds share. So a file read takes no request of its own, once its extent is mapped. A region
 * goes with its last slice. A write to a slice's chunk drops the slice (-EXDEV), and the chunk is
 * mapped again as a range of its own: a store to the region's page would change it for every file
 * sharing it.
 *
 * Ranges belong to a file (struct fxfs_dax_file, by inode number), not to an inode: they outlive
 * the inodes read in for the file, so that a file read again maps from the window what it mapped
 * before, with no request, as pmem stays mapped across the guest's dropped caches. A file's ranges
 * go as its data does (unlinked, truncated); others are reclaimed, least recently used, when the
 * window runs out.
 *
 * Locks are taken in this order: an inode's lock (read and write); fxdax: a follower's fill
 * barrier (vfs.h), which a batch takes holding no inode's lock; an inode's invalidate_lock
 * (faults, truncation and reclaim); its file's writeback lock (`wb_lock`); then its file's DAX
 * semaphore. Another inode's are only tried, when reclaiming its ranges.
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
#include <linux/sort.h>
#include <linux/uio.h>
#include <linux/vmalloc.h>
#include <linux/virtio_extent.h>
#include <linux/wait.h>
#include <linux/workqueue.h>
#include <linux/writeback.h>

#include "device.h"
#include "fxfs_trace.h"
#include "hist.h"
#include "pages.h"
#include "vfs.h"

/* A range's size: a power of two, the window's and chunks' alignment. */
#define FXFS_DAX_SHIFT 21
#define FXFS_DAX_SIZE (1ULL << FXFS_DAX_SHIFT)
#define FXFS_DAX_PAGES (FXFS_DAX_SIZE >> PAGE_SHIFT)
/*
 * fxdax: the most pages one write's iomap makes private: an eighth of the device's private_max, if
 * less (fxfs_dax_iomap_write).
 */
#define FXFS_DAX_PRIVATE_PAGES 64UL
/*
 * fxdax: the pages a write(2) following on from its file's last makes private ahead of it, at
 * first and at most (fxfs_dax_write_follows, TASK-164.10).
 */
#define FXFS_DAX_AHEAD_FIRST 16U
#define FXFS_DAX_AHEAD_MOST 64U
/* fxdax: how far back in the last write a write may begin and follow on from it: a log's block. */
#define FXFS_DAX_FOLLOWS (16UL << 10)

/*
 * The window's space is handed out from a pool, page by page, at this offset: the pool takes no
 * address 0.
 */
#define FXFS_DAX_POOL_BASE (1UL << 48)

/* fxdax: how many waits of 10 ms with nothing let go of a range waits for room for (30 s). */
#define FXFS_DAX_NO_ROOM_WAITS 3000
/*
 * fxdax: how many times a write or a fault has a slice dropped, its chunk mapped again as a range
 * of its own, before giving up, as a reader may make it a slice again each time; and how many
 * reads of a range copy nothing, memory there, before one gives up.
 */
#define FXFS_DAX_MAX_DROPS 256

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

/* fxdax: fault injection: inodes fail to attach to their files, which leaves them in the page cache. */
static bool fxfs_dax_fail_attach;
module_param_named(dax_fail_attach, fxfs_dax_fail_attach, bool, 0644);
MODULE_PARM_DESC(dax_fail_attach, "Fail inodes' attaching to their files, which leaves them in the page cache (fault injection)");

/*
 * fxdax: fault injection: of the next this many maps of a range's pages in more than one piece, the
 * range maps the first piece alone, then fails (fxfs_dax_map_all).
 */
static unsigned int fxfs_dax_fail_maps;
module_param_named(dax_fail_maps, fxfs_dax_fail_maps, uint, 0644);
MODULE_PARM_DESC(dax_fail_maps, "Fail ranges' maps of more than one piece after the first, the next this many (fault injection)");

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
 * fxdax: pause points (TASK-119.9): a task reaching a step whose bit fxfs.dax_pause sets waits
 * there until the bit is cleared, the bit set in fxfs.dax_paused meanwhile: a check plays an
 * interleaving out exactly, where fxfs.dax_delay_us's random sleeps only make it likely. The bits
 * of the steps writable DAX had (0 to 3, and 6) are unused, so that the others keep theirs.
 */
enum fxfs_dax_step {
	/* A read of a file, its iomap made, before the copy of what it maps, holding its range. */
	FXFS_DAX_STEP_READ_MAPPED = 4,
	/*
	 * A write(2) of a file, its iomap made, its pages dirty, before the copy into what it maps,
	 * holding the file's semaphore shared (fxfs_dax_iomap_write).
	 */
	FXFS_DAX_STEP_WRITE_MAPPED = 5,
	/*
	 * A rebind about to MAP a run of a file's clean private pages over, holding the file's
	 * writeback lock and its semaphore for writing (fxfs_dax_rebind).
	 */
	FXFS_DAX_STEP_REBIND = 7,
	/*
	 * A writeback, its dirty set swapped out, before it reads the set's pages from the window,
	 * holding their ranges and the file's writeback lock (fxfs_dax_write_back).
	 */
	FXFS_DAX_STEP_WRITEBACK = 8,
	/*
	 * A write fault past its iomap_end, its PTE writable and the file's semaphore let go, holding
	 * its page's DAX entry until it returns (fxfs_dax_protect).
	 */
	FXFS_DAX_STEP_FAULTED = 9,
	/*
	 * A writeback, its file's dirty ranges counted, its batch allocated for them, before it holds
	 * the semaphore for writing to swap them out, holding the writeback lock alone
	 * (fxfs_dax_write_back).
	 */
	FXFS_DAX_STEP_COUNTED = 10,
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

/* fxdax: waits at `step`, of file `ino`, while fxfs.dax_pause asks, if fxfs.dax_pause_ino names it. */
static void fxfs_dax_pause_file(enum fxfs_dax_step step, u64 ino)
{
	unsigned long only = READ_ONCE(fxfs_dax_pause_ino);

	if (likely(!(READ_ONCE(fxfs_dax_pause_steps) & BIT(step))) || (only && only != ino))
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

/* The window's mapping of a whole sealed extent, from its start, which slices of files share. */
struct fxfs_dax_region {
	/* In the filesystem's regions, by extent. */
	struct hlist_node node;
	u64 branch;
	u64 sequence;
	u64 window_offset;
	/* Its space in the window, in pages: as far as the extent reaches. */
	unsigned long pages;
	/* Its slices. Guarded by the regions lock. */
	unsigned int refs;
	/* Guards `mapped`, and the region's MAPs. */
	struct mutex lock;
	/* How many of its pages are mapped, from its start. */
	unsigned long mapped;
};

/* A mounted volume's share of the window: the files of its inode numbers, which are its own. */
struct fxfs_dax_volume {
	struct fxfs *fs;
	/* The files with ranges, or inodes using the window, by inode number. */
	struct xarray files;
	/* On the filesystem's volumes. Guarded by its files lock. */
	struct list_head list;
	/*
	 * fxdax: the page-cache inodes whose writeback wrote since the last seal began, by inode
	 * number, each the seal count at which a barrier will have sealed it (fxfs_dax_written,
	 * fxfs_dax_seal_target): read in before then, an inode is a page-cache inode again, as the
	 * window would find its extents open, which it maps none of (doc-13, TASK-164.3). The worker
	 * prunes it as seals come.
	 */
	struct xarray unsealed;
};

/* A file's ranges of the window, which outlive its inodes. */
struct fxfs_dax_file {
	/* Its volume, until the volume goes (fxfs_dax_remove_volume). Guarded by the files lock. */
	struct fxfs_dax_volume *volume;
	u64 ino;
	/* The filesystem's (`files`), and others' while they use it. */
	refcount_t refs;
	/* The inode read in for the file, if there is one. Guarded by the filesystem's files lock. */
	struct inode *inode;
	/* Guards `ranges`, and their pages' state. */
	struct rw_semaphore sem;
	/* The window's ranges mapping chunks of the file, by chunk. */
	struct xarray ranges;
	/*
	 * fxdax: a write whose chunk a reader made a slice again, after the write had it dropped, under
	 * the inode's write lock: its chunk gets a range of its own, so that no slice can refuse it
	 * again (xfstests generic/638).
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
	 * fxdax: held by the file's writeback, from before it swaps the dirty set out until Fxfs has
	 * committed the pages it wrote, which rebinding and truncation wait for: a writeback's pages,
	 * its writeback set, are those it holds it for. Taken before the semaphore (doc-13).
	 */
	struct mutex wb_lock;
	/* Its ranges' pages in its dirty set, and counted private (fxfs_dax_count). */
	atomic_long_t dirty_pages, private_pages;
	/*
	 * fxdax: when its pages were last made dirty or private, in jiffies, which the budget rebinds
	 * the files of, longest ago first (fxfs_dax_relieve, TASK-164.10).
	 */
	unsigned long dirtied;
	/*
	 * fxdax: the last write(2)'s bytes, and how many pages past its end the next, following on
	 * from it, makes private ahead of it (fxfs_dax_iomap_write): under the inode's write lock,
	 * which write(2) holds (TASK-164.10).
	 */
	loff_t write_pos, write_end;
	unsigned int ahead;
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
	/* The iomaps and writebacks using it, which keep it. */
	atomic_t users;
	/*
	 * fxdax: its pages' state, under its file's semaphore (pages.h): which map something, an
	 * extent's page or zeros; which the host holds private, from the PRIVATE of the iomap that
	 * dirtied them (fxfs_dax_dirty) to the MAP or UNMAP over them; and which are in its file's
	 * dirty set, until writeback swaps them out (fxfs_dax_write_back).
	 */
	struct fxfs_dax_pages *state;
	/*
	 * fxdax: whether a MAP of its pages failed (fxfs_dax_map_all), which may have mapped some of
	 * them all the same, as a MAP_MANY's requests before the one that failed did: its state says
	 * nothing of those, which its UNMAP takes nonetheless (fxfs_dax_unmap_range). Under its
	 * file's semaphore held for writing.
	 */
	bool unsure;
	/* Freed after a grace period: the stats walk files' ranges without their semaphores. */
	struct rcu_head rcu;
};

/*
 * fxdax: why the window maps: a region as far as a slice needs, or pages in a range's own space
 * (TASK-91.4), both for the access at hand, which the device maps ahead of (MAP_AHEAD, TASK-112.1).
 */
enum fxfs_dax_why {
	FXFS_DAX_REACH,
	FXFS_DAX_RANGE,
	FXFS_DAX_WHYS,
};

static const char *const fxfs_dax_whys[FXFS_DAX_WHYS] = {
	"reach", "range",
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
	 * fxdax: whether the device follows a store another VM writes (FXFS_DEVICE_READ_ONLY): its
	 * window takes no store, nor maps zeros, its holes left unmapped.
	 */
	bool read_only;
	/*
	 * fxdax: how often the window had no room for a range, or for a region in their own space,
	 * how many ranges were reclaimed for room, and how many regions there are (TASK-91.7);
	 * how many chunks' ranges took room off a chunk's alignment, which no PMD maps (TASK-149.33).
	 */
	atomic_long_t window_full, regions_full, reclaims, region_count, unaligned;
	/* fxdax: the window's MAPs, and the bytes they mapped, by why (`enum fxfs_dax_why`). */
	atomic64_t maps[FXFS_DAX_WHYS], map_bytes[FXFS_DAX_WHYS];
	/* fxdax: the window's bytes kept from use as their UNMAP failed (TASK-44.22). */
	atomic64_t lost_bytes;
	/* fxdax: the faults whose iomap mapped a PMD's 2 MiB, its pages in a row (TASK-123). */
	atomic64_t huge_maps;
	/* Requested bytes of extent-result buffers, before allocator rounding. */
	atomic64_t extent_alloc_bytes;
	/*
	 * fxdax: where write(2)'s time goes (TASK-146.2): in all, waiting for the inode's lock, what
	 * comes before its first iomap, faulting in its source, dax_iomap_rw (of which its iomaps'
	 * begin and end, the rest being the copy, a store's faults on the window among it), dropping
	 * a slice to try again, and what comes after.
	 */
	atomic64_t writes, write_ns, write_lock_ns, write_setup_ns, write_fault_in_ns, write_rw_ns;
	/* fxdax: a write's time making its pages private before its lock (TASK-164.11). */
	atomic64_t write_private_ns;
	atomic64_t write_begin_ns, write_end_ns, write_retry_ns, write_tail_ns;
	struct fxfs_hist write_hist;
	/* fxdax: what each fault of a mapping took (TASK-111.6). */
	struct fxfs_hist fault_hist;
	/* The volumes mounted, each with its files. */
	struct list_head volumes;
	/* Guards `volumes`, their files, and files' inodes. */
	struct mutex files_lock;
	/* Guards the used list. */
	spinlock_t lock;
	/* The ranges mapping chunks, least recently used first. */
	struct list_head used;
	/* Woken as ranges are freed, or their last users done, which counts in `released`. */
	wait_queue_head_t wait;
	atomic_long_t released;
	/* The regions mapping extents, by extent. */
	DECLARE_HASHTABLE(regions, 12);
	/* Guards `regions`, and regions' references. */
	spinlock_t regions_lock;
	/* Where the budget's worker runs. */
	struct workqueue_struct *wq;
	/*
	 * fxdax: the most pages the device holds private at once, which the guest keeps under, and the
	 * pages counted private, and in files' dirty sets (fxfs_dax_count).
	 */
	long private_max;
	atomic_long_t private, dirty;
	/* fxdax: the most pages one write's iomap makes private: a slice of the budget. */
	unsigned long iomap_pages;
	/*
	 * fxdax: the budget's worker (fxfs_dax_work): writes back dirty files, and rebinds clean
	 * private pages, as the private pages near private_max, the window runs out of room, or a
	 * test asks; its rounds, which those it throttles wait for (`budget_wait`); whether it is to
	 * write back whatever is dirty (`clean`), as asked since its last round; the asks to rebind
	 * everything it can (fxfs.dax_rebind_now), and those its rounds have served, which a check
	 * waits for; and whether it stops, the filesystem unmounting or freezing, and whether it is
	 * frozen, when it writes nothing.
	 */
	struct work_struct work;
	wait_queue_head_t budget_wait;
	atomic64_t rounds;
	atomic_t clean;
	atomic64_t rebinds_asked, rebinds_done;
	bool stopping, frozen;
	/* fxdax: the page-cache inodes in the volumes' `unsealed`, which the worker prunes. */
	atomic_long_t unsealed;
	/*
	 * fxdax: the writebacks, the pages they wrote, the pages rebound, the iomaps the budget held
	 * up, the seals made for a range or the budget, and the PRIVATE requests of the iomaps that
	 * dirtied pages (TASK-164.2, TASK-164.9).
	 */
	atomic64_t writebacks, written, rebound, throttled, seals_made, privates;
	/*
	 * fxdax: the pages PRIVATEs made private ahead of writes (TASK-164.10), and before writes
	 * took their inodes' locks (TASK-164.11).
	 */
	atomic64_t private_ahead, private_before;
	/*
	 * fxdax: MAPs the device refused as of an open extent (-EPERM), which Fxfs had taken for
	 * sealed: none is meant to be asked for (doc-13's invariant 1), a tripwire the checks read.
	 */
	atomic64_t open_maps;
};

/* Each of `dax`'s files, of each volume, with its files lock held. */
#define fxfs_dax_for_each_file(dax, volume, ino, file)			\
	list_for_each_entry(volume, &(dax)->volumes, list)		\
		xa_for_each(&(volume)->files, ino, file)

static const struct iomap_ops fxfs_dax_iomap_ops;

/*
 * fxdax: the `count` maps `maps`, in as few requests as the device takes: or, with
 * fxfs.dax_fail_map_many, the first alone, then a failure, as a device's MAP_MANY may.
 */
static int fxfs_dax_device_map_many(struct fxfs_dax *dax, const struct fxfs_device_map *maps,
				    unsigned int count)
{
	int err;

	if (count > 1 && fxfs_dax_inject(&fxfs_dax_fail_map_many)) {
		err = fxfs_device_map_many(dax->dev, maps, 1);
		return err ?: -EIO;
	}
	return fxfs_device_map_many(dax->dev, maps, count);
}

/* Where `range`'s page `page` is in the window. */
static u64 fxfs_dax_page_offset(const struct fxfs_dax_range *range, unsigned long page)
{
	return range->window_offset + ((u64)page << PAGE_SHIFT);
}

static const struct address_space_operations fxfs_dax_aops;
static const struct file_operations fxfs_dax_file_operations;
static const struct fxfs_data_ops fxfs_dax_data_ops;
static int fxfs_dax_break_layouts(struct inode *inode, loff_t start);
static int fxfs_dax_zero_tail(struct inode *inode, loff_t old, loff_t size);
static void fxfs_dax_put_region(struct fxfs_dax *dax, struct fxfs_dax_region *region);

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
 * fxdax: diagnostic (TASK-121): with fxfs.dax_trace set as the filesystem mounts, the latest of
 * what happened to files' pages, which writing `ino page` to fxfs.dax_trace_dump logs for that
 * page: where the window maps it, and the iomaps of it.
 */
enum fxfs_dax_trace_kind {
	FXFS_DAX_TRACE_MAPPED,
	FXFS_DAX_TRACE_WRITE,
	FXFS_DAX_TRACE_FAULT,
	FXFS_DAX_TRACE_READ,
};

static const char *const fxfs_dax_trace_kinds[] = {
	"mapped", "write iomap", "fault iomap", "read iomap",
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
 * fxdax: whether `range` holds pages an UNMAP, or a MAP over them, would lose: those of its file's
 * dirty set, which no extent holds yet. Under its file's semaphore.
 */
static bool fxfs_dax_range_dirty(const struct fxfs_dax_range *range)
{
	return fxfs_dax_pages_any_dirty(range->state);
}

/* fxdax: counts `private` more pages private, and `dirty` more dirty, of `file`'s, or fewer. */
static void fxfs_dax_count(struct fxfs_dax *dax, struct fxfs_dax_file *file, long private,
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
		wake_up_all(&dax->budget_wait);
}

/*
 * fxdax: has the budget's worker run a round (fxfs_dax_work), which writes back and rebinds as the
 * private pages near private_max: and write back whatever is dirty, if `clean`, as a window whose
 * ranges are all dirty has nothing to reclaim. Not while it stops. It only queues the round.
 */
static void fxfs_dax_kick(struct fxfs_dax *dax, bool clean)
{
	if (clean)
		atomic_set(&dax->clean, 1);
	if (!READ_ONCE(dax->stopping))
		queue_work(dax->wq, &dax->work);
}

/*
 * fxdax: counts what a transition of pages whose data goes took (doc-13): an UNMAP, or a
 * truncate's MAP of zeros, ended `took`'s private pages of `range`: their dirty pages' room
 * released, dropped unwritten, as a truncate's, or an unlinked file's are; and, once the host has
 * freed them (`freed`), their private pages no longer counted. Not before the request ends: the
 * budget let another's store have the room, which the host, holding the pages still, refused past
 * private_max (guest/sim, seed 90). Nor if it failed, the host holding them for good: counted
 * still, lost, as the window's pages an UNMAP failed for are (fxfs_dax_give_unmapped). With its
 * file's semaphore held for writing.
 */
static void fxfs_dax_dropped(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			     struct fxfs_dax_change took, bool freed)
{
	struct fxfs_dax_file *file = range->file;
	struct fxfs_dax_volume *volume;

	fxfs_dax_count(dax, file, freed ? took.private : 0, took.dirty);
	if (!freed)
		atomic_long_add(took.private, &file->private_pages);
	/* A volume going (fxfs_dax_remove_volume) was synced: nothing of it is dirty. */
	volume = READ_ONCE(file->volume);
	if (took.dirty && !WARN_ON_ONCE(!volume))
		fxfs_release_dirty(volume->fs, file->ino, (u64)-took.dirty << PAGE_SHIFT);
}

/* fxdax: traces what `map`, of `range`'s pages, mapped: an extent's pages, or zeros. */
static void fxfs_dax_trace_map(const struct fxfs_dax_range *range,
			       const struct fxfs_device_map *map)
{
	unsigned long page = (map->window_offset - range->window_offset) >> PAGE_SHIFT;
	unsigned long end = page + (map->len >> PAGE_SHIFT), i;
	u64 first = fxfs_dax_chunk_start(range) >> PAGE_SHIFT;

	for (i = page; i < end; i++)
		fxfs_dax_trace(FXFS_DAX_TRACE_MAPPED, range->file->ino, first + i,
			       fxfs_dax_page_offset(range, i), map->zeros ? 0 : map->sequence,
			       map->zeros ? 0 : map->offset + ((u64)(i - page) << PAGE_SHIFT),
			       map->zeros);
}

/*
 * fxdax: the transition `map`, of `range`'s pages, made, once it mapped them: a range's own map,
 * of pages it maps nothing at (fxfs_dax_pages_fill); or a rebind's (`rebind`), of private, clean
 * pages, written back and sealed, private no more, the host having freed them for the extent's
 * page, or zeros, holding the same bytes (fxfs_dax_pages_rebind), counted: how many it rebound.
 * With the file's semaphore held for writing.
 */
static long fxfs_dax_mapped(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			    const struct fxfs_device_map *map, bool rebind)
{
	unsigned long page = (map->window_offset - range->window_offset) >> PAGE_SHIFT;
	unsigned long n = map->len >> PAGE_SHIFT;
	long rebound;

	lockdep_assert_held_write(&range->file->sem);
	fxfs_dax_trace_map(range, map);
	if (!rebind) {
		fxfs_dax_pages_fill(range->state, page, n, map->zeros);
		return 0;
	}
	rebound = fxfs_dax_pages_rebind(range->state, page, n, map->zeros);
	if (rebound)
		fxfs_dax_count(dax, range->file, -rebound, 0);
	return rebound;
}

/*
 * fxdax: maps the `count` maps `maps`, of `range`'s pages, in as few requests as the device takes,
 * or one by one if that fails, to say which: a sealed extent the device doesn't hold (-ENOENT),
 * never written, as zeros, a hole, as Fxfs reads it, or, a follower's, left unmapped, a hole. An
 * extent open still (-EPERM: the window maps none), Fxfs having taken it for sealed, as one minted
 * before a seal and written after it is (storage_device's OpenExtents), fails them, unless a
 * `rebind`'s, when it is left as it is, its pages private. Has the range say what it mapped, and a
 * rebind's pages private no more (fxfs_dax_mapped): how many that rebound, or the first error. A
 * range's own maps are of pages it maps nothing at, none private. With its file's semaphore held
 * for writing. fxdax: with fxfs.dax_fail_maps, a map of a range's pages (not a rebind's) of more
 * than one piece maps the first, then fails, as a device's MAP of the second may; with
 * fxfs.dax_fail_fallbacks, the MAPs one by one after a failed MAP_MANY fail past the first
 * (TASK-164.2).
 */
static long fxfs_dax_map_all(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			     struct fxfs_device_map *maps, size_t count, bool rebind)
{
	long rebound = 0;
	size_t i;
	int err;

	if (!count)
		return 0;
	if (count > 1 && !rebind && fxfs_dax_inject(&fxfs_dax_fail_maps)) {
		rebound = fxfs_dax_map_all(dax, range, maps, 1, rebind);
		range->unsure = true;
		return rebound < 0 ? rebound : -EIO;
	}
	if (trace_fxfs_dax_map_enabled()) {
		u64 len = 0;

		for (i = 0; i < count; i++)
			len += maps[i].len;
		trace_fxfs_dax_map(rebind ? FXFS_TRACE_MAP_REBIND : FXFS_TRACE_MAP_RANGE,
				   range->file->ino, maps[0].window_offset, len, count);
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
				  fxfs_device_map(dax->dev, map->branch, map->sequence, map->offset,
						  map->len, map->window_offset, map->ahead);

			if (one == -ENOENT && !map->zeros && dax->read_only)
				continue;
			if (one == -ENOENT && !map->zeros) {
				map->zeros = true;
				one = fxfs_device_map_zeros(dax->dev, map->window_offset, map->len);
			}
			if (one == -EPERM)
				atomic64_inc(&dax->open_maps);
			if (one == -EPERM && rebind)
				continue;
			if (one) {
				pr_warn("fxfs: mapping %llu bytes at %llu of %s at %llu of the DAX window failed: %d\n",
					map->len, map->offset, map->zeros ? "zeros" : "an extent",
					map->window_offset, one);
				/* What the MAP_MANY mapped of those after it, it doesn't say. */
				range->unsure = true;
				return one;
			}
		}
		atomic64_add(map->len, &dax->map_bytes[FXFS_DAX_RANGE]);
		rebound += fxfs_dax_mapped(dax, range, map, rebind);
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
 * fxdax: the pieces of file `ino`'s `len` bytes at `offset`, as fxfs_dax_extents, once no seal was
 * in progress as Fxfs looked: one says sealed from its beginning of what it has yet to seal on the
 * device (OpenExtents::seal, as fxfs-linux's Seal::begin_with has it), which the host refuses to
 * map (-EPERM). Looked up again after it, as long as seals keep coming: mapping them, a range's
 * map sealed and tried again, and a rebind left them private, in the cow_ scenarios' schedules a
 * mutant moved (cow_map_during_seal). A slice's region, likewise (fxfs_dax_new_slice).
 */
static ssize_t fxfs_dax_sealed_extents(struct fxfs_dax *dax, struct fxfs *fs, u64 ino,
				       loff_t offset, u64 len, struct fxfs_dax_extent *found,
				       size_t capacity)
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
 * fxdax: the maps of `range`'s pages `first` to `last`, exclusive, in `maps`, from the pieces
 * `found` of the file's bytes there, `count` of them, sorted, which may reach past them: each
 * sealed piece's pages from its extent, zeros where the file has none, a hole or past its end, but
 * on a follower's window, which leaves those unmapped. Maps ahead (MAP_AHEAD) if `ahead`. As many
 * as 2 * (`last` - `first`) + 1 at most: how many.
 */
static size_t fxfs_dax_maps_of(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			       const struct fxfs_dax_extent *found, ssize_t count,
			       unsigned long first, unsigned long last, bool ahead,
			       struct fxfs_device_map *maps)
{
	loff_t start = fxfs_dax_chunk_start(range);
	unsigned long page = first;
	size_t n = 0;
	ssize_t i;

	for (i = 0; i <= count; i++) {
		const struct fxfs_dax_extent *e = i < count ? &found[i] : NULL;
		unsigned long from = last, to;
		u64 at;

		if (e)
			from = clamp_t(unsigned long, (e->offset - start) >> PAGE_SHIFT, page, last);
		/* The hole before it, or past the last: zeros. */
		if (from > page && !dax->read_only)
			maps[n++] = (struct fxfs_device_map) {
				.window_offset = fxfs_dax_page_offset(range, page),
				.len = (u64)(from - page) << PAGE_SHIFT,
				.zeros = true,
			};
		if (!e || from >= last)
			break;
		to = clamp_t(unsigned long, (e->offset + e->len - start) >> PAGE_SHIFT, from, last);
		at = start + ((u64)from << PAGE_SHIFT);
		if (to > from && e->access == FXFS_DAX_SEALED)
			maps[n++] = (struct fxfs_device_map) {
				.branch = e->branch, .sequence = e->sequence,
				.offset = e->extent_offset + (at - e->offset),
				.len = (u64)(to - from) << PAGE_SHIFT,
				.window_offset = fxfs_dax_page_offset(range, from),
				.ahead = ahead,
			};
		page = max(page, to);
	}
	return n;
}

/*
 * fxdax: maps `range`'s pages `first` to `last`, exclusive, read-only (doc-13): each from the
 * sealed extent holding the file's bytes there, or zeros where it has none, holes and pages past
 * its end, so that every page of a range has a window page, which the iomap that dirties it makes
 * private, and no write is new (IOMAP_F_NEW); a follower's window leaves those unmapped, holes, as
 * it takes no store. Nothing maps an open extent: what is open is sealed first (fxfs_fsync), as a
 * page-cache inode's writes read in before their seal, or a growth's copy of the last block, may
 * leave; rebinding private pages, as their writeback left them, is fxfs_dax_rebind_range's, which
 * leaves those whose extent is open private. 0, or a -errno. With the file's semaphore held for
 * writing.
 */
static long fxfs_dax_map_pages(struct fxfs_dax *dax, struct fxfs *fs, struct fxfs_dax_range *range,
			       unsigned long first, unsigned long last)
{
	loff_t start = fxfs_dax_chunk_start(range);
	size_t capacity = last - first, n;
	struct fxfs_dax_extent *found;
	struct fxfs_device_map *maps;
	unsigned int tries;
	ssize_t count;
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
		count = fxfs_dax_sealed_extents(dax, fs, range->file->ino,
						start + ((loff_t)first << PAGE_SHIFT),
						(u64)capacity << PAGE_SHIFT, found, capacity);
		if (count < 0) {
			ret = count;
			goto out;
		}
		if (!fxfs_dax_any_unsealed(found, count)) {
			n = fxfs_dax_maps_of(dax, range, found, count, first, last, true, maps);
			ret = fxfs_dax_map_all(dax, range, maps, n, false);
			if (ret != -EPERM)
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
		atomic64_inc(&dax->seals_made);
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
 * fxdax: rebinds `range`'s private, clean pages (fxfs_dax_rebind): every run of them in as few
 * requests as the device takes (MAP_MANY), from one look at the file's pieces over them all, where
 * each run was a look and a MAP of its own. A forked pgbench's writes scattered over its tables
 * left runs of a page or two: 30,085 MAPs a minute, 294 us each, nested, the file's semaphore held
 * for writing through all of a file's (TASK-164.10, GCE r649). A page whose extent is open yet
 * stays private (fxfs_dax_map_all), counted in `*open`. How many it rebound, or a -errno. With the
 * file's semaphore held for writing, and its writeback lock.
 */
static long fxfs_dax_rebind_range(struct fxfs_dax *dax, struct fxfs *fs,
				  struct fxfs_dax_range *range, unsigned long *open)
{
	loff_t start = fxfs_dax_chunk_start(range);
	unsigned long first, last = 0, page, end, rebindable = 0;
	struct fxfs_dax_extent *found;
	struct fxfs_device_map *maps;
	size_t capacity, runs = 0, n = 0;
	ssize_t count, i = 0;
	long ret;

	lockdep_assert_held_write(&range->file->sem);
	first = fxfs_dax_pages_next_rebindable(range->state, 0, &end);
	for (page = first; page < range->pages;
	     page = fxfs_dax_pages_next_rebindable(range->state, end, &end)) {
		last = end;
		rebindable += end - page;
		runs++;
	}
	if (!runs)
		return 0;
	capacity = last - first;
	found = fxfs_dax_alloc_extents(dax, capacity);
	/* A run's maps are twice its pages and one at most. */
	maps = kmalloc_array(2 * capacity + runs, sizeof(*maps), GFP_NOFS);
	if (!found || !maps) {
		ret = -ENOMEM;
		goto out;
	}
	/* Before the MAP over them, holding what a rebind holds (pause points). */
	fxfs_dax_pause_file(FXFS_DAX_STEP_REBIND, range->file->ino);
	count = fxfs_dax_sealed_extents(dax, fs, range->file->ino,
					start + ((loff_t)first << PAGE_SHIFT),
					(u64)capacity << PAGE_SHIFT, found, capacity);
	if (count < 0) {
		ret = count;
		goto out;
	}
	for (page = fxfs_dax_pages_next_rebindable(range->state, first, &end); page < range->pages;
	     page = fxfs_dax_pages_next_rebindable(range->state, end, &end)) {
		/* The pieces before the run, done with: the runs come in order. */
		while (i < count &&
		       found[i].offset + found[i].len <= start + ((u64)page << PAGE_SHIFT))
			i++;
		n += fxfs_dax_maps_of(dax, range, found + i, count - i, page, end, false, maps + n);
	}
	ret = fxfs_dax_map_all(dax, range, maps, n, true);
	/* Every page clean and private under the semaphore is rebound, but those of open extents. */
	if (ret >= 0)
		*open += rebindable - ret;
out:
	kfree(maps);
	kvfree(found);
	return ret;
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
	fxfs_dax_pages_free(range->state);
	kfree(range);
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
	range->state = fxfs_dax_pages_new(pages);
	if (!range->state) {
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
	spin_unlock(&dax->regions_lock);
	if (region->mapped)
		err = fxfs_device_unmap(dax->dev, region->window_offset,
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
	bool mapped = !range->region &&
		      (range->unsure || fxfs_dax_pages_any_mapped(range->state, 0, range->pages));
	int err = 0;

	lockdep_assert_held_write(&range->file->sem);
	WARN(atomic_read(&range->users),
	     "fxfs: ino %llu chunk %lu's range at %llx of the DAX window freed, %d using it\n",
	     range->file->ino, range->chunk, range->window_offset, atomic_read(&range->users));

	xa_erase(&range->file->ranges, range->chunk);
	if (mapped)
		err = fxfs_device_unmap(dax->dev, range->window_offset,
					(u64)range->pages << PAGE_SHIFT);
	/* fxdax: the UNMAP freed its private pages, unless it failed. */
	fxfs_dax_dropped(dax, range, fxfs_dax_pages_unmap(range->state, 0, range->pages), !err);
	fxfs_dax_free_range(dax, range, err);
}

/* Drops a reference to `file`, freeing it once it has none. */
static void fxfs_dax_put_file(struct fxfs_dax *dax, struct fxfs_dax_file *file)
{
	if (!refcount_dec_and_test(&file->refs))
		return;
	xa_destroy(&file->ranges);
	kfree(file);
}

/*
 * Forgets `file` if no inode is read in for it and it has no ranges, with the files lock held:
 * returns whether it did, when the caller drops the filesystem's reference to it.
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
 * fxdax: cleans the kernel's dirty tags of `inode`'s DAX entries from `start` to `end`, for the
 * kernel's sake alone: a write fault tags its page's entry, and nothing of dax.c reads the tags,
 * its dirty sets saying what to write back, and its writeback write-protecting by the page tables
 * (fxfs_dax_write_back). But a mapping tagged dirty keeps its inode dirty
 * (__writeback_single_inode sets I_DIRTY_PAGES again), so the flusher's for ever and off the inode
 * LRU (writepages); and invalidate_inode_pages2_range refuses an entry tagged so
 * (fxfs_dax_release). An entry a fault holds, dax_writeback_one passes by, tagged still, which the
 * next writepages, or the range's release, with no fault under way, cleans. Its PTEs are
 * write-protected with it: a store after faults again, its page dirty already.
 */
static int fxfs_dax_clean_entries(struct inode *inode, loff_t start, loff_t end)
{
	struct writeback_control clean = {
		.sync_mode = WB_SYNC_ALL,
		.nr_to_write = LONG_MAX,
		.range_start = start,
		.range_end = end,
	};

	return dax_writeback_mapping_range(inode->i_mapping, fxfs_dax_of(inode)->dax_dev, &clean);
}

/*
 * Removes the inode's mappings of the chunk `range` maps, and the DAX entries for them, unless
 * a page is in use (pinned): with the inode's invalidate_lock held for writing. fxdax: and its
 * file's semaphore. Unless the range holds dirty pages, which its UNMAP would lose: their
 * writeback (fxfs_dax_write_back) takes the semaphore.
 */
static bool fxfs_dax_release(struct inode *inode, struct fxfs_dax_range *range)
{
	struct address_space *mapping = inode->i_mapping;
	loff_t start = fxfs_dax_chunk_start(range);
	loff_t end = start + ((loff_t)range->pages << PAGE_SHIFT) - 1;

	if (fxfs_dax_range_dirty(range))
		return false;
	if (dax_layout_busy_page_range(mapping, start, end))
		return false;
	/*
	 * Dirty entries go only once cleaned. fxdax: with no page dirty, an entry tagged dirty is the
	 * kernel's alone, cleaned here, which the invalidate_lock makes whole, no fault holding an
	 * entry: one a writeback left tagged, a fault holding it, failed the invalidation for ever,
	 * and a fault waiting for room reclaimed the range again and again (guest/sim, seed 3390 at
	 * 1,000 operations).
	 */
	if (fxfs_dax_clean_entries(inode, start, end))
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
		/* fxdax: an evicted inode's were written back: kept if not. */
		if (range && !atomic_read(&range->users) && !fxfs_dax_range_dirty(range)) {
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
	trace_fxfs_dax_reclaim(file->ino, chunk, reclaimed);
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
		 * fxdax: nothing to reclaim, a chunk's range takes the room the window has off a
		 * chunk's alignment, mapped a page at a time (fs/dax.c falls back from a PMD where
		 * the window's page isn't 2 MiB-aligned): UNMAPs that failed keep a piece of an
		 * aligned slot from use for good, and every chunk's write failed ENOSPC once each
		 * slot had one, most of the window free (TASK-149.33).
		 */
		if (aligned && pages == FXFS_DAX_PAGES) {
			window_offset = fxfs_dax_take_window(dax, pages, false);
			if (window_offset >= 0) {
				atomic_long_inc(&dax->unaligned);
				return window_offset;
			}
		}
		/* fxdax: ranges holding dirty pages aren't reclaimed: written back, they are. */
		fxfs_dax_kick(dax, true);
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
	range = fxfs_dax_new_range(window_offset, pages, NULL);
	if (IS_ERR(range))
		fxfs_dax_give_window(dax, window_offset, pages);
	return range;
}

/*
 * The bytes the region of extent `e` spans: the extent's, or a chunk's if Fxfs doesn't count it, as
 * a volume over a base counts none of the base's, whose filesystem's allocator does: as far as a
 * slice's extent may reach, a small file's staging extent's (fxfs_dax_new_slice). The device maps
 * zeros past an extent's end, so that the region maps whole in one MAP all the same, which each of
 * the files of the extent then slices: refused, each was a range of its own, a MAP each, and a
 * sandbox's cold read of the Linux tree over an attached generation took 27 s, 19.6 of them in
 * 91,072 MAPs, against a fresh VM's 5.7 s on the publisher's store, its 1,005 (GCE r940,
 * TASK-229). A piece past it is a range's of its own, as fxfs_dax_region_map refuses it (-EXDEV).
 */
static u64 fxfs_dax_region_size(const struct fxfs_dax_extent *e)
{
	return e->extent_size ?: FXFS_DAX_SIZE;
}

/*
 * The region mapping extent `e`'s, made, unmapped, if there is none: with a reference more, the
 * caller's. -EAGAIN if the window has no room for it now, reclaiming ranges for it if regions
 * share the ranges' space: the caller holds a file's DAX semaphore, and can't wait for room.
 * fxdax: in a space of their own, regions reclaim nothing: their callers map what they would have
 * in ranges' (TASK-91.10).
 */
static struct fxfs_dax_region *fxfs_dax_get_region(struct fxfs_dax *dax,
						   const struct fxfs_dax_extent *e)
{
	unsigned long pages = DIV_ROUND_UP(fxfs_dax_region_size(e), PAGE_SIZE);
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
		window_offset = fxfs_dax_alloc_window(dax, pages, false, true);
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
 * Maps `region`'s sealed extent as far as page `end` at least, read-only: all of it if it can, so
 * that the next slices need no MAP; else as far as `end`, as a sealed extent may end before what
 * was allocated of it, past what was written. -EXDEV if the region's space doesn't reach so far.
 */
static int fxfs_dax_region_map(struct fxfs_dax *dax, struct fxfs_dax_region *region,
			       unsigned long end)
{
	unsigned long from, to;
	int err = 0;

	mutex_lock(&region->lock);
	from = region->mapped;
	if (end <= from)
		goto out;
	if (end > region->pages) {
		err = -EXDEV;
		goto out;
	}
	to = region->pages;
	for (;;) {
		trace_fxfs_dax_map(FXFS_TRACE_MAP_REACH, 0,
				   region->window_offset + ((u64)from << PAGE_SHIFT),
				   (u64)(to - from) << PAGE_SHIFT, 1);
		err = fxfs_device_map(dax->dev, region->branch, region->sequence,
				      (u64)from << PAGE_SHIFT, (u64)(to - from) << PAGE_SHIFT,
				      region->window_offset + ((u64)from << PAGE_SHIFT), true);
		fxfs_dax_delay();
		atomic64_inc(&dax->maps[FXFS_DAX_REACH]);
		atomic64_add((u64)(to - from) << PAGE_SHIFT, &dax->map_bytes[FXFS_DAX_REACH]);
		if (!err || to == end)
			break;
		to = end;
	}
	/*
	 * An extent the device doesn't hold (-ENOENT) maps nothing: its callers read its pages as
	 * holes, which the region must not claim as mapped, or a slice of it would hand the guest
	 * window pages the host never mapped. One open still (-EPERM), which Fxfs had taken for
	 * sealed, is mapped by a range of its own, which seals it first.
	 */
	if (!err)
		WRITE_ONCE(region->mapped, to);
	else if (err == -EPERM)
		atomic64_inc(&dax->open_maps);
	else if (err != -ENOENT)
		pr_warn("fxfs: mapping pages %lu to %lu of extent %llx:%llu, of %lu pages, at %llu of the DAX window failed: %d\n",
			from, to, region->branch, region->sequence, region->pages,
			region->window_offset, err);
out:
	mutex_unlock(&region->lock);
	return err;
}

/*
 * Makes `inode`'s chunk `chunk` a slice of its extent's region, if its data in the file's `size`
 * bytes is one piece of a sealed extent the window maps whole, and returns it, kept, in its file's
 * ranges. NULL if it isn't such a piece, or the window has no room for its region now. With its
 * file's DAX semaphore held for writing.
 */
static struct fxfs_dax_range *fxfs_dax_new_slice(struct fxfs_dax *dax, struct inode *inode,
						 pgoff_t chunk, loff_t size)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	loff_t start = (loff_t)chunk << FXFS_DAX_SHIFT;
	loff_t end = min_t(loff_t, start + FXFS_DAX_SIZE, round_up(size, PAGE_SIZE));
	struct fxfs_dax_region *region;
	struct fxfs_dax_range *range;
	struct fxfs_dax_extent e;
	unsigned long first, pages;
	ssize_t count;
	int err;

	lockdep_assert_held_write(&file->sem);

	if (end <= start || READ_ONCE(file->no_slices))
		return NULL;
	pages = (end - start) >> PAGE_SHIFT;
	count = fxfs_dax_sealed_extents(dax, fxfs_vfs_fs(inode->i_sb), inode->i_ino, start,
					end - start, &e, 1);
	fxfs_dax_delay();
	/* Holes, or more than one piece. */
	if (count == 0 || count == -E2BIG)
		return NULL;
	if (count < 0)
		return ERR_PTR(count);
	/* Nothing maps an open extent: a range of its own maps the chunk, sealing it first. */
	if (e.offset != start || e.len != end - start || fxfs_dax_region_size(&e) > FXFS_DAX_SIZE ||
	    !PAGE_ALIGNED(e.extent_offset) ||
	    e.access != FXFS_DAX_SEALED)
		return NULL;
	region = fxfs_dax_get_region(dax, &e);
	if (IS_ERR(region))
		return PTR_ERR(region) == -EAGAIN ? NULL : ERR_CAST(region);
	first = e.extent_offset >> PAGE_SHIFT;
	err = fxfs_dax_region_map(dax, region, first + pages);
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
	/* Its pages are its region's, mapped. */
	fxfs_dax_pages_fill(range->state, 0, pages, false);
	err = xa_err(xa_store(&file->ranges, chunk, range, GFP_NOFS));
	if (err) {
		/* Its slice goes with it. */
		fxfs_dax_free_range(dax, range, 0);
		return ERR_PTR(err);
	}
	spin_lock(&dax->lock);
	list_add_tail(&range->list, &dax->used);
	spin_unlock(&dax->lock);
	return range;
out:
	fxfs_dax_put_region(dax, region);
	/*
	 * Its region can't reach it, its extent was never written, or is open still: a range of its
	 * own maps it, or the holes it is.
	 */
	return err == -EXDEV || err == -ENOENT || err == -EPERM ? NULL : ERR_PTR(err);
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
 * bytes long, kept for an iomap: a slice of its extent's region if it can be and `slice` allows,
 * or a range of its own, every page of it mapped (fxfs_dax_map_pages). A write's chunk is a range
 * of its own (doc-13). For a fault (`fault`), -EAGAIN rather than wait for room in the window.
 */
static struct fxfs_dax_range *fxfs_dax_get_range(struct inode *inode, pgoff_t chunk, loff_t size,
						 bool fault, bool slice)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range, *new;
	unsigned long pages;
	long mapped;
	int err;

	down_read(&file->sem);
	range = xa_load(&file->ranges, chunk);
	if (range)
		atomic_inc(&range->users);
	up_read(&file->sem);
	if (range)
		goto used;

	down_write(&file->sem);
	range = xa_load(&file->ranges, chunk);
	if (range)
		atomic_inc(&range->users);
	else if (slice)
		range = fxfs_dax_new_slice(dax, inode, chunk, max3(i_size_read(inode), size,
								   READ_ONCE(file->growing)));
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
	down_write(&file->sem);
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
		/* Every page of it, zeros past the file's end (doc-13). */
		mapped = fxfs_dax_map_pages(dax, fxfs_vfs_fs(inode->i_sb), new, 0, new->pages);
		err = mapped < 0 ? mapped : 0;
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

static void fxfs_dax_put_range(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	if (atomic_dec_and_test(&range->users))
		fxfs_dax_released(dax);
}

/*
 * Drops `inode`'s ranges of chunks `first` to `last` too small for the file to be `size` bytes
 * long, or all of them if `size` is negative, once nothing uses their pages: they are mapped again
 * as needed. A range's dirty pages are written back first: the UNMAP frees their private pages,
 * and the range maps again what Fxfs then has. With the inode's invalidate_lock held for writing.
 * fxdax: unless `pinned`, -EBUSY rather than wait for a page pinned (TASK-79.49).
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
			dirty = fxfs_dax_range_dirty(range);
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
			err = fxfs_dax_write_back(inode);
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
 * Drops `inode`'s range of the chunk at `pos`, a slice a write can't use (-EXDEV), to be mapped
 * again as a range of its own. fxdax: unless `pinned`, -EBUSY rather than wait for pages pinned
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

/* Fills in `iomap` for a hole of `length` bytes. */
static void fxfs_dax_hole(struct iomap *iomap, loff_t length)
{
	iomap->type = IOMAP_HOLE;
	iomap->addr = IOMAP_NULL_ADDR;
	iomap->length = length;
}

/*
 * fxdax: maps those of `range`'s pages `first` to `last`, exclusive, that have no window page, as
 * a truncate's MAP of zeros that failed leaves the pages past its end (fxfs_dax_resize_end), which
 * a growth brings back into `inode`, so that every page of the file has one (fxfs_dax_map_pages):
 * 0 holding the file's semaphore shared, as an iomap does, or a -errno holding nothing of it.
 * Before any iomap of such a page, a read's as a write's: a read's hole has a fault map the
 * kernel's zero page, which nothing takes away, no write being new (IOMAP_F_NEW), so that a load
 * read zeros where read(2) read a write's bytes, and a write from the mapping copied them (guest/sim,
 * failures seed 1669). Not a follower's, whose holes have none, as it takes no write.
 */
static int fxfs_dax_map_missing(struct inode *inode, struct fxfs_dax_range *range,
				unsigned long first, unsigned long last)
{
	struct fxfs_dax_file *file = range->file;
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	unsigned long page, to;
	long mapped;
	int err;

	for (;;) {
		down_read(&file->sem);
		if (dax->read_only ||
		    fxfs_dax_pages_next_unmapped(range->state, first, last) == last)
			return 0;
		up_read(&file->sem);
		down_write(&file->sem);
		err = 0;
		while (!err &&
		       (page = fxfs_dax_pages_next_unmapped(range->state, first, last)) < last) {
			to = fxfs_dax_pages_mapped_run(range->state, page, last);
			mapped = fxfs_dax_map_pages(dax, fxfs_vfs_fs(inode->i_sb), range, page, to);
			err = mapped < 0 ? mapped : 0;
		}
		up_write(&file->sem);
		if (err)
			return err;
	}
}

/*
 * Maps a read of up to `length` bytes at `pos`: a run of mapped pages, or of holes.
 */
static int fxfs_dax_iomap_read(struct inode *inode, loff_t pos, loff_t length, unsigned int flags,
			       struct iomap *iomap)
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
	down_read(&file->sem);
	range = xa_load(&file->ranges, pos >> FXFS_DAX_SHIFT);
	if (range) {
		atomic_inc(&range->users);
		fxfs_dax_use_range(dax, range);
	} else {
		up_read(&file->sem);
		range = fxfs_dax_get_range(inode, pos >> FXFS_DAX_SHIFT, 0, fault, true);
		if (IS_ERR(range))
			return PTR_ERR(range);
		down_read(&file->sem);
	}
	/* fxdax: the window between a follower's extents read and its mapping, widened in tests. */
	if (fxfs_vfs_follows(inode->i_sb) && fxfs_follow_iomap_delay_us) {
		up_read(&file->sem);
		fsleep(fxfs_follow_iomap_delay_us);
		down_read(&file->sem);
	}
	start = fxfs_dax_chunk_start(range);
	page = (pos - start) >> PAGE_SHIFT;
	/* A range covers the file's chunk as its size grows (fxfs_dax_fit). */
	if (WARN_ON_ONCE(page >= range->pages)) {
		up_read(&file->sem);
		fxfs_dax_put_range(dax, range);
		return -EIO;
	}
	/* fxdax: a page of the file with no window page is no hole: mapped first. */
	if (!dax->read_only && !fxfs_dax_pages_mapped(range->state, page)) {
		up_read(&file->sem);
		err = fxfs_dax_map_missing(inode, range, page,
					   min_t(u64, range->pages,
						 page + DIV_ROUND_UP(offset_in_page(pos) + length,
								     PAGE_SIZE)));
		if (err) {
			fxfs_dax_put_range(dax, range);
			return err;
		}
	}
	mapped = fxfs_dax_pages_mapped(range->state, page);
	end_page = fxfs_dax_pages_mapped_run(range->state, page, range->pages);
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
	if (fault) {
		fxfs_dax_trace(FXFS_DAX_TRACE_READ, inode->i_ino, (start >> PAGE_SHIFT) + page, at, 0,
			       length >> PAGE_SHIFT, 0);
		if (length >= PMD_SIZE && IS_ALIGNED(at, PMD_SIZE))
			atomic64_inc(&dax->huge_maps);
		/*
		 * fxdax: a slice's page may be in other files' DAX entries too, or this one's at
		 * another offset, as blocks of a staging extent or a reflink's are: said so, as XFS
		 * says of its reflinks' reads, or the kernel takes the page for one no entry has
		 * (dax_associate_entry's WARN, xfstests generic/075, TASK-140).
		 */
		if (range->region)
			iomap->flags |= IOMAP_F_SHARED;
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

/*
 * fxdax: the window of the filesystem mounted last, for fxfs.dax_stray and the tests' knobs; it
 * goes as the mount does.
 */
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
 * fxdax: for tests (doc-13's stage 2): every this many milliseconds, the budget's worker writes
 * back, seals and rebinds every private page it can, under whatever stores go on; 0 never. Setting
 * it starts it, on the filesystem mounted last.
 */
static unsigned int fxfs_dax_rebind_every;

/*
 * fxdax: a writeback's pages of a range: the dirty set it swapped out, and those of it that Fxfs
 * had no memory to write, which it keeps (TASK-207).
 */
struct fxfs_dax_batch {
	struct fxfs_dax_range *range;
	unsigned long pages[BITS_TO_LONGS(FXFS_DAX_PAGES)];
	unsigned long kept[BITS_TO_LONGS(FXFS_DAX_PAGES)];
};

/* fxdax: the pages one part of a writeback's run writes: `end - page` from `page` of a range's. */
struct fxfs_dax_run_part {
	size_t batch;
	unsigned long page, end;
};

/*
 * fxdax: the most pages one fxfs_write_many writes back: a megabyte, as one of the page cache's
 * writeback's (FXFS_IO_MAX, super.c).
 */
#define FXFS_DAX_WRITEBACK_RUN 256

/*
 * fxdax: has Fxfs write the `nr` parts `parts` of file `ino`, `pages` dirty pages, paid for with
 * their room; or, with no part, of pages past the file's end, releases it.
 */
static int fxfs_dax_write_run(struct fxfs *fs, u64 ino, const struct fxfs_write_part *parts,
			      size_t nr, unsigned long pages)
{
	if (nr)
		return fxfs_nofs(fxfs_write_many(fs, ino, parts, nr, (u64)pages << PAGE_SHIFT));
	if (pages)
		fxfs_release_dirty(fs, ino, (u64)pages << PAGE_SHIFT);
	return 0;
}

/*
 * fxdax: what a run of a writeback's, its `n` parts' pages `run` of `batch`'s, came to as Fxfs
 * wrote it with `err`: an error whose pages are lost, as the page cache's are after an I/O error;
 * or 0, if it wrote them, or had no memory to, which keeps them, in their batches' `kept`, `*kept`
 * counting them, for a writeback later to write (TASK-207).
 */
static int fxfs_dax_ran(struct fxfs_dax_batch *batch, const struct fxfs_dax_run_part *run,
			size_t n, int err, unsigned long *kept)
{
	size_t i;

	if (err != -ENOMEM)
		return err;
	for (i = 0; i < n; i++) {
		bitmap_set(batch[run[i].batch].kept, run[i].page, run[i].end - run[i].page);
		*kept += run[i].end - run[i].page;
	}
	return 0;
}

/*
 * fxdax: has Fxfs write the `nr` ranges' pages `batch`, of `inode`, from the window, where stores
 * made them private, to new extents, as the page cache's writeback writes folios
 * (fxfs_vfs_writepages): FXFS_DAX_WRITEBACK_RUN pages a call at most, each with the room its
 * dirtying reserved, up to the file's end. Fxfs copies them into its transfer buffer, checksums
 * that, and commits after the bytes; a store meanwhile reaches the copy or not, its page dirty
 * again either way. `parts` and `run` have room for FXFS_DAX_WRITEBACK_RUN. The first error of a
 * run whose pages are lost, the rest written still; a run Fxfs had no memory for keeps its pages,
 * `*kept` of them (fxfs_dax_run_failed).
 */
static int fxfs_dax_write_pages(struct fxfs_dax *dax, struct inode *inode,
				struct fxfs_dax_batch *batch, size_t nr,
				struct fxfs_write_part *parts, struct fxfs_dax_run_part *run_parts,
				unsigned long *kept)
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
				  page + FXFS_DAX_WRITEBACK_RUN - run);
			had = dax_direct_access(dax->dax_dev,
						fxfs_dax_page_offset(range, page) >> PAGE_SHIFT,
						end - page, DAX_ACCESS, &kaddr, NULL);
			/* Past the file's end, as a write that copied nothing there leaves: nothing. */
			if (pos < size && had >= (long)(end - page)) {
				run_parts[n] = (struct fxfs_dax_run_part) {
					.batch = i, .page = page, .end = end,
				};
				parts[n++] = (struct fxfs_write_part) {
					.offset = pos,
					.data = kaddr,
					.len = min_t(loff_t, (loff_t)(end - page) << PAGE_SHIFT,
						     size - pos),
				};
			} else if (pos < size) {
				err = err ?: -EIO;
			}
			run += end - page;
			if (run == FXFS_DAX_WRITEBACK_RUN) {
				ran = fxfs_dax_write_run(fs, inode->i_ino, parts, n, run);
				/* Every run's kept, whatever came before. */
				ran = fxfs_dax_ran(batch, run_parts, n, ran, kept);
				err = err ?: ran;
				n = 0;
				run = 0;
			}
		}
	}
	ran = fxfs_dax_write_run(fs, inode->i_ino, parts, n, run);
	dax_read_unlock(id);
	ran = fxfs_dax_ran(batch, run_parts, n, ran, kept);
	return err ?: ran;
}

/*
 * fxdax: makes the user's mappings of the `nr` ranges' pages `batch`, a writeback's, read-only
 * (wp_shared_mapping_range): a store to one after this faults, and dirties its page again. By the
 * page tables, which see every PTE, not the DAX entries: a fault past its iomap, which made its
 * page dirty and its PTE writable, holds the page's entry until it returns, after its iomap_end
 * let the semaphore go and a writeback swapped the page out; and dax_writeback_mapping_range,
 * finding the entry locked, gives up on it rather than wait (dax_writeback_one's
 * get_next_unlocked_entry, mid-walk, says it gone). That PTE stayed writable: a rebind mapped the
 * page read-only under it, which the host refuses a store to, or a store through it after the
 * writeback's copy went in no writeback (guest/sim, seed 22).
 */
static void fxfs_dax_protect(struct address_space *mapping, const struct fxfs_dax_batch *batch,
			     size_t nr)
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
 * fxdax: a writeback's pages Fxfs had no memory to write (`batch`'s `kept`), dirty again in their
 * file's dirty set, their room reserved again, which the writeback took (fxfs_write_many), what
 * there is of it, so that a writeback later writes them (TASK-207). Clean, as the page cache's
 * after an I/O error, they were lost: the private pages read as written, and a clone, which takes
 * the store's blocks, a rebind or an eviction brought back the blocks they replaced (GCE r699).
 * With the writeback lock held, which keeps their ranges from rebinding meanwhile. How many it
 * made dirty: not those a truncate or an UNMAP took meanwhile, nor those a store dirtied again.
 */
static unsigned long fxfs_dax_keep(struct inode *inode, struct fxfs_dax_batch *batch, size_t nr)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	unsigned long made = 0;
	unsigned int nofs;
	size_t i;

	down_write(&file->sem);
	for (i = 0; i < nr; i++)
		made += fxfs_dax_pages_redirty(batch[i].range->state, batch[i].kept);
	fxfs_dax_count(fxfs_dax_of(inode), file, 0, made);
	up_write(&file->sem);
	if (!made)
		return 0;
	nofs = memalloc_nofs_save();
	fxfs_reserve_dirty_what_fits(fxfs_vfs_fs(inode->i_sb), inode->i_ino,
				     (u64)made << PAGE_SHIFT);
	memalloc_nofs_restore(nofs);
	/* For the flusher's writepages, as fxfs_dax_dirty marks it. */
	__mark_inode_dirty(inode, I_DIRTY_PAGES);
	return made;
}

/*
 * fxdax: the writeback transition (doc-13): writes back `inode`'s dirty pages, as fsync, msync,
 * sync, the flusher (writepages), the budget, an UNMAP of a range, a size's change and eviction
 * need, from dax.c's dirty set alone. Under its file's semaphore held for writing, so that no
 * write's iomap is between dirtying a page and its store or PTE, the dirty set is swapped for an
 * empty one: the writeback set, each page of it the VM's own on the host since the iomap that
 * dirtied it (fxfs_dax_dirty), so that its block may move under it. The writeback lock (`wb_lock`)
 * keeps the set from rebinding until Fxfs has committed it. Then what faults made writable is
 * write-protected (fxfs_dax_protect), so that a store after faults, and dirties its page again;
 * and the set's bytes are written to new extents, which Fxfs allocates now, committing after them
 * (fxfs_dax_write_pages). Swapped under the semaphore shared, a fault between its iomap and its PTE
 * left a writable PTE to a page the set took, which a rebind then lost the stores of. Fxfs takes a
 * file's size as its writeback writes its pages, as the page cache's. fxdax: pages Fxfs had no
 * memory to write stay dirty if `keep` (fxfs_dax_keep, TASK-207), the writeback failing ENOMEM;
 * an I/O error's are clean, the file's error set, as the page cache's.
 */
/* fxdax: how many of `file`'s ranges hold dirty pages: with its semaphore held, shared at least. */
static size_t fxfs_dax_dirty_ranges(struct fxfs_dax_file *file)
{
	struct fxfs_dax_range *range;
	unsigned long chunk;
	size_t dirty = 0;

	xa_for_each(&file->ranges, chunk, range)
		dirty += fxfs_dax_range_dirty(range);
	return dirty;
}

static int fxfs_dax_write_back_keeping(struct inode *inode, bool keep)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_batch *batch = NULL;
	struct fxfs_write_part *parts = NULL;
	struct fxfs_dax_run_part *run_parts = NULL;
	unsigned long kept = 0;
	struct fxfs_dax_range *range;
	unsigned long chunk;
	size_t nr = 0, most, now, i;
	long pages = 0, written = 0;
	bool traced = false;
	int err = 0;

	if (!IS_DAX(inode) || !file)
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
	traced = true;
	trace_fxfs_dax_writeback_start(file->ino);
	parts = kmalloc_array(FXFS_DAX_WRITEBACK_RUN, sizeof(*parts), GFP_NOFS);
	run_parts = kmalloc_array(FXFS_DAX_WRITEBACK_RUN, sizeof(*run_parts), GFP_NOFS);
	if (!parts || !run_parts) {
		err = -ENOMEM;
		goto out;
	}
	down_read(&file->sem);
	most = fxfs_dax_dirty_ranges(file);
	up_read(&file->sem);
	/*
	 * The batch, before the swap, as many as the semaphore held shared counted: a writeback that
	 * can't write leaves the set dirty. Counted again once it is held for writing, which keeps
	 * iomaps from dirtying more, and made larger if they did beside the count: one too small passed
	 * by the ranges last in the file, dirty before an fsync began, which returned 0 without them
	 * (guest/sim's file model, failures seed 62).
	 */
	for (;;) {
		if (!most)
			goto out;
		batch = fxfs_nofs(kvmalloc_array(most, sizeof(*batch), GFP_KERNEL));
		if (!batch) {
			err = -ENOMEM;
			goto out;
		}
		/* Between the count and the swap, where iomaps may dirty more (pause points). */
		fxfs_dax_pause_file(FXFS_DAX_STEP_COUNTED, inode->i_ino);
		down_write(&file->sem);
		now = fxfs_dax_dirty_ranges(file);
		if (now && now <= most)
			break;
		up_write(&file->sem);
		kvfree(batch);
		batch = NULL;
		most = now;
	}
	xa_for_each(&file->ranges, chunk, range) {
		if (!fxfs_dax_range_dirty(range))
			continue;
		batch[nr].range = range;
		/* Kept, mapped, while the writeback reads it: no reclaim, no drop. */
		atomic_inc(&range->users);
		pages += fxfs_dax_pages_swap(range->state, batch[nr].pages);
		bitmap_zero(batch[nr].kept, FXFS_DAX_PAGES);
		nr++;
	}
	fxfs_dax_count(dax, file, 0, -pages);
	up_write(&file->sem);
	fxfs_dax_protect(inode->i_mapping, batch, nr);
	/* Before it reads its ranges' window pages, which nothing may free (pause points). */
	fxfs_dax_pause_file(FXFS_DAX_STEP_WRITEBACK, inode->i_ino);
	err = fxfs_dax_write_pages(dax, inode, batch, nr, parts, run_parts, &kept);
	if (kept && !keep) {
		err = err ?: -ENOMEM;
		kept = 0;
	} else if (kept) {
		kept = fxfs_dax_keep(inode, batch, nr);
	}
	for (i = 0; i < nr; i++)
		fxfs_dax_put_range(dax, batch[i].range);
	atomic64_inc(&dax->writebacks);
	written = pages - kept;
	atomic64_add(written, &dax->written);
	/*
	 * As the page cache's writeback: what an I/O error stopped is clean, the error the file's
	 * (fsync's, syncfs's). What Fxfs had no memory for is dirty still (TASK-207), and this
	 * writeback says so, ENOMEM, the file's error not set: the next writes it.
	 */
	if (err)
		mapping_set_error(inode->i_mapping, err);
	if (kept)
		err = err ?: -ENOMEM;
out:
	if (traced)
		trace_fxfs_dax_writeback_end(file->ino, err ?: written);
	mutex_unlock(&file->wb_lock);
	kfree(run_parts);
	kfree(parts);
	kvfree(batch);
	return err;
}

int fxfs_dax_write_back(struct inode *inode)
{
	return fxfs_dax_write_back_keeping(inode, true);
}


/*
 * fxdax: the rebinding transition (doc-13): rebinds `file`'s clean private pages, those in no dirty
 * set, nor in a writeback's under way, which the writeback lock waits for, each MAPped to the
 * sealed extent's page holding its bytes, as Fxfs has them, or to zeros where it has none: the
 * same bytes at the same address, the host's private page freed, nothing in the guest changed, and
 * the page private no more (fxfs_dax_mapped). Under the file's semaphore held for writing, which
 * sees each with no writable PTE (the fault that made one dirtied its page, which its writeback
 * write-protected) and no copy into it under way (invariant 3). A page whose extent is open yet,
 * its writeback's barrier not come, stays private. Range by range, until no more than `target`
 * pages are private: the writeback lock and the semaphore taken for each range, which held through
 * a file's every range kept its readers, writers and fsyncs waiting through every MAP of them
 * (TASK-164.10). How many it rebound, and in `*open` how many it left private, their extents open.
 */
static long fxfs_dax_rebind_ranges(struct fxfs_dax *dax, struct fxfs_dax_file *file, long target,
				   unsigned long *open)
{
	struct fxfs_dax_volume *volume;
	struct fxfs_dax_range *range;
	unsigned long chunk = 0;
	long rebound = 0, done;

	while (atomic_long_read(&file->private_pages) && atomic_long_read(&dax->private) > target) {
		mutex_lock(&file->wb_lock);
		down_write(&file->sem);
		/* A volume going (fxfs_dax_remove_volume) stops the worker first. */
		volume = READ_ONCE(file->volume);
		range = volume ? xa_find(&file->ranges, &chunk, ULONG_MAX, XA_PRESENT) : NULL;
		if (range) {
			done = fxfs_dax_rebind_range(dax, volume->fs, range, open);
			if (done < 0)
				pr_warn_ratelimited("fxfs: rebinding ino %llu chunk %lu: %ld\n",
						    file->ino, chunk, done);
			else
				rebound += done;
		}
		up_write(&file->sem);
		mutex_unlock(&file->wb_lock);
		if (!range || chunk == ULONG_MAX)
			break;
		chunk++;
		cond_resched();
	}
	return rebound;
}

/*
 * fxdax: rebinds `file`'s clean private pages (fxfs_dax_rebind_ranges) until no more than `target`
 * are private; and, if it left some private, their extents open, with more private than that still,
 * seals, and rebinds again. Writeback outside the budget's (a growth's or a truncate's, a write's
 * that stopped short, a drop's, the flusher's) seals nothing after it: what it wrote stays open
 * until the next fsync or sync seals it, and the worker sealed only what it wrote back itself. With
 * nothing dirty, as after interleave seed 215500's write stopped short, every round rebound none,
 * and a write's or a punch's wait for room failed ENOMEM (TASK-283). How many it rebound.
 */
static long fxfs_dax_rebind(struct fxfs_dax *dax, struct fxfs_dax_file *file, long target)
{
	struct fxfs_dax_volume *volume;
	unsigned long open = 0;
	long rebound;
	int err;

	trace_fxfs_dax_rebind_start(file->ino);
	rebound = fxfs_dax_rebind_ranges(dax, file, target, &open);
	/* A volume going (fxfs_dax_remove_volume) stops the worker first. */
	volume = READ_ONCE(file->volume);
	if (open && volume && atomic_long_read(&dax->private) > target) {
		atomic64_inc(&dax->seals_made);
		err = fxfs_nofs(fxfs_fsync(volume->fs));
		if (err)
			pr_warn_ratelimited("fxfs: sealing ino %llu's private pages: %d\n", file->ino,
					    err);
		else
			rebound += fxfs_dax_rebind_ranges(dax, file, target, &open);
	}
	atomic64_add(rebound, &dax->rebound);
	trace_fxfs_dax_rebind_end(file->ino, rebound);
	return rebound;
}

/*
 * fxdax: references to every volume's files with dirty or private pages, in `*files`, which the
 * caller frees, putting each: how many, or -ENOMEM without the memory for them, when a sync
 * mustn't say it wrote them (guest/sim's sweep: it returned 0, and the files' writes were lost).
 */
static ssize_t fxfs_dax_busy_files(struct fxfs_dax *dax, struct fxfs_dax_file ***files)
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
	return most && !*files ? -ENOMEM : nr;
}

/* fxdax: the inode read in for `file`, with a reference, if there is one. */
static struct inode *fxfs_dax_inode_of(struct fxfs_dax *dax, struct fxfs_dax_file *file)
{
	struct inode *inode = NULL;

	mutex_lock(&dax->files_lock);
	if (file->inode)
		inode = igrab(file->inode);
	mutex_unlock(&dax->files_lock);
	return inode;
}

/*
 * fxdax: orders files by when their pages were last made dirty or private (`dirtied`), the
 * longest ago first: the budget rebinds those first, where it went by inode number and rebound a
 * pgbench's log file, being written, its pages made private again at its next write (TASK-164.10).
 */
static int fxfs_dax_dirtied_before(const void *a, const void *b)
{
	unsigned long x = READ_ONCE((*(struct fxfs_dax_file *const *)a)->dirtied);
	unsigned long y = READ_ONCE((*(struct fxfs_dax_file *const *)b)->dirtied);

	return time_before(x, y) ? -1 : time_after(x, y) ? 1 : 0;
}

/*
 * fxdax: relieves the private pages (doc-13's budget): writes back every file with dirty pages,
 * if `write`, not while frozen; seals what that wrote, for its pages' extents to map; and rebinds
 * clean private pages, file by file, until no more than `target` are private.
 */
static void fxfs_dax_relieve(struct fxfs_dax *dax, long target, bool write)
{
	struct fxfs_dax_file **files;
	ssize_t nr = fxfs_dax_busy_files(dax, &files), i;
	struct fxfs *wrote = NULL;
	int err;

	/* Without the memory, the next round relieves them. */
	if (nr < 0)
		return;

	for (i = 0; write && i < nr; i++) {
		struct fxfs_dax_volume *volume = READ_ONCE(files[i]->volume);
		struct inode *inode;

		if (!volume || !atomic_long_read(&files[i]->dirty_pages))
			continue;
		inode = fxfs_dax_inode_of(dax, files[i]);
		if (!inode)
			continue;
		err = fxfs_dax_write_back(inode);
		if (err)
			pr_warn_ratelimited("fxfs: writing back ino %lu's private pages: %d\n",
					    inode->i_ino, err);
		iput(inode);
		wrote = volume->fs;
	}
	/* The barrier after them, which the journal's flush makes, seals them. */
	if (wrote) {
		atomic64_inc(&dax->seals_made);
		err = fxfs_nofs(fxfs_fsync(wrote));
		if (err)
			pr_warn_ratelimited("fxfs: sealing what the budget wrote back: %d\n", err);
	}
	/* The files dirtied longest ago first, which are least likely to be written again soon. */
	sort(files, nr, sizeof(*files), fxfs_dax_dirtied_before, NULL);
	for (i = 0; i < nr; i++) {
		if (atomic_long_read(&dax->private) > target)
			fxfs_dax_rebind(dax, files[i], target);
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
static void fxfs_dax_prune(struct fxfs_dax *dax)
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
 * files and rebinds clean private pages, to three quarters; asked to (`clean`, as the window runs
 * out of room for ranges), it writes back whatever is dirty; asked to rebind all
 * (fxfs.dax_rebind_now, fxfs.dax_rebind_every), as many as it can. Then it prunes the page-cache
 * inodes sealed since, says which asks to rebind all it served, those made before it began, and
 * wakes those the budget held up. The window's reclaim learns of ranges its writeback cleaned as
 * it lets go of them (fxfs_dax_put_range), not of a round: woken by every round, which each wait
 * for room asked for, a write that could never have room, its window's aligned room lost to UNMAPs
 * that failed, found something let go of each time, and waited for ever (guest/sim's sweep,
 * read_pressure).
 */
static void fxfs_dax_work(struct work_struct *work)
{
	struct fxfs_dax *dax = container_of(work, struct fxfs_dax, work);
	unsigned int every = READ_ONCE(fxfs_dax_rebind_every);
	s64 asked = atomic64_read(&dax->rebinds_asked);
	bool clean = atomic_cmpxchg(&dax->clean, 1, 0) == 1;
	bool all = asked != atomic64_read(&dax->rebinds_done) || every;
	long max = dax->private_max;

	if (clean || all || atomic_long_read(&dax->private) > max - max / 8)
		fxfs_dax_relieve(dax, all ? 0 : max - max / 4, !READ_ONCE(dax->frozen));
	fxfs_dax_prune(dax);
	atomic64_set(&dax->rebinds_done, asked);
	atomic64_inc(&dax->rounds);
	wake_up_all(&dax->budget_wait);
	if (every && !READ_ONCE(dax->stopping)) {
		msleep(every);
		fxfs_dax_kick(dax, false);
	}
}

/*
 * fxdax: counts `pages` more pages private, as the iomap that dirties them has the host make them
 * so, if private_max has room for them; else none, and false, for the iomap to wait for the worker
 * (doc-13's budget). Past seven eighths of it, the worker writes back and rebinds.
 */
static bool fxfs_dax_take_private(struct fxfs_dax *dax, long pages)
{
	long now = atomic_long_add_return(pages, &dax->private);

	if (now > dax->private_max) {
		atomic_long_sub(pages, &dax->private);
		atomic64_inc(&dax->throttled);
		fxfs_dax_kick(dax, false);
		return false;
	}
	if (now > dax->private_max - dax->private_max / 8)
		fxfs_dax_kick(dax, false);
	return true;
}

/*
 * fxdax: waits, holding nothing of the file's, for a round of the worker's since a write's iomap
 * found no room among the private pages (-ENOBUFS), as balance_dirty_pages waits after a fault: 0
 * to try again, -EINTR if killed, or -ENOMEM once FXFS_DAX_NO_ROOM_WAITS rounds or waits of 10 ms
 * made none, as for a range.
 */
static int fxfs_dax_throttle(struct fxfs_dax *dax, unsigned int *waits)
{
	s64 round = atomic64_read(&dax->rounds);
	long waited;

	fxfs_dax_kick(dax, false);
	waited = wait_event_killable_timeout(dax->budget_wait,
					     atomic64_read(&dax->rounds) != round,
					     msecs_to_jiffies(10));
	if (waited < 0)
		return -EINTR;
	if (++*waits > FXFS_DAX_NO_ROOM_WAITS)
		return -ENOMEM;
	return 0;
}

/*
 * fxdax: a PRIVATE of `pages` pages at `offset` of the window failed with `err`: the `counted`
 * pages counted private for it given back, and the error its write fails with: ENOMEM as ENOMEM,
 * the guest's request's memory not found, not EIO (TASK-44.15); the host's refusal is EIO already
 * (fxfs_device_private).
 */
static int fxfs_dax_private_failed(struct fxfs_dax *dax, u64 offset, unsigned long pages,
				   long counted, int err)
{
	pr_warn_ratelimited("fxfs: making %lu pages at %llx of the window private: %d\n", pages,
			    offset, err);
	atomic_long_sub(counted, &dax->private);
	wake_up_all(&dax->budget_wait);
	return err == -ENOMEM ? err : -EIO;
}

/*
 * fxdax: the dirtying transition (doc-13), the one way a page joins its file's dirty set:
 * `range`'s `pages` pages from `first`, a write's (a fault's, a write(2)'s or zeroing's), about to
 * be stored to. Each not dirty yet gets room in Fxfs for its writeback (a fault without it fails
 * SIGBUS, a write(2) ENOSPC, as the page cache's do; zeroing, a truncate's or a punch's, takes
 * what there is). Each not private yet is counted private, within private_max, else -ENOBUFS, for
 * the caller to wait for the budget's worker, holding nothing (fxfs_dax_throttle); and the host
 * makes them the VM's own now (PRIVATE), holding what they held, before they are dirty: so a dirty
 * page is private, whenever its store lands. A fault's store lands after its iomap_end, maybe after
 * a writeback has taken its page, moved its block, and had Fxfs free the old one: the host copying
 * the page at the store, as it would, copied the zeros the window reads there then (guest/sim,
 * seed 2802 at 1,000 operations). A failed PRIVATE fails the write, -EIO, or -ENOMEM if the guest
 * found no memory for its request, nothing dirty, the room given back; what the host made private
 * of it stays so, uncounted until a write makes it dirty, or a MAP or UNMAP frees it. With the
 * file's semaphore held shared, which the caller holds until the store or the copy is done
 * (fxfs_dax_iomap_end): no writeback swaps the page out, nor a rebind maps over it, meanwhile.
 *
 * A PRIVATE takes `ahead` pages more, after them, private and clean, if there is room for them
 * below where the budget's worker begins (fxfs_dax_take_private), and the write's own pages need
 * it: those the stores following on from the write's, a log's, reach next (fxfs_dax_iomap_write),
 * which take no round trip then (TASK-164.10).
 */
static int fxfs_dax_dirty(struct inode *inode, struct fxfs_dax_range *range, unsigned long first,
			  unsigned long pages, bool zero, unsigned long ahead)
{
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs *fs = fxfs_vfs_fs(inode->i_sb);
	struct fxfs_dax_change would, made;
	long fresh, newly, beyond = 0, ahead_made = 0;
	int err = 0;

	lockdep_assert_held_read(&range->file->sem);
	would = fxfs_dax_pages_would_dirty(range->state, first, pages);
	fresh = would.dirty;
	newly = would.private;
	if (fresh && zero) {
		unsigned int nofs = memalloc_nofs_save();

		fxfs_reserve_dirty_what_fits(fs, inode->i_ino, (u64)fresh << PAGE_SHIFT);
		memalloc_nofs_restore(nofs);
	} else if (fresh) {
		err = fxfs_nofs(fxfs_reserve_dirty(fs, inode->i_ino, (u64)fresh << PAGE_SHIFT));
		if (err)
			return err;
	}
	if (newly && ahead) {
		beyond = fxfs_dax_pages_would_dirty(range->state, first + pages, ahead).private;
		if (atomic_long_read(&dax->private) + newly + beyond >
		    dax->private_max - dax->private_max / 8)
			ahead = beyond = 0;
	} else {
		ahead = 0;
	}
	if (newly && !fxfs_dax_take_private(dax, newly + beyond)) {
		err = -ENOBUFS;
		goto unreserve;
	}
	if (newly) {
		/* The pages private already, those others' iomaps made so meanwhile too, stay so. */
		err = fxfs_device_private(dax->dev, fxfs_dax_page_offset(range, first),
					  (u64)(pages + ahead) << PAGE_SHIFT);
		atomic64_inc(&dax->privates);
		if (err) {
			err = fxfs_dax_private_failed(dax, fxfs_dax_page_offset(range, first),
						      pages + ahead, newly + beyond, err);
			goto unreserve;
		}
		WRITE_ONCE(range->file->dirtied, jiffies);
	}
	made = fxfs_dax_pages_dirty(range->state, first, pages);
	if (ahead) {
		ahead_made = fxfs_dax_pages_private_ahead(range->state, first + pages, ahead);
		atomic64_add(ahead_made, &dax->private_ahead);
	}
	/* Others' iomaps, holding the semaphore shared too, may have counted some meanwhile. */
	if (newly + beyond > made.private + ahead_made)
		atomic_long_sub(newly + beyond - made.private - ahead_made, &dax->private);
	atomic_long_add(made.private + ahead_made, &range->file->private_pages);
	if (fresh > made.dirty)
		fxfs_release_dirty(fs, inode->i_ino, (u64)(fresh - made.dirty) << PAGE_SHIFT);
	if (made.dirty) {
		atomic_long_add(made.dirty, &dax->dirty);
		atomic_long_add(made.dirty, &range->file->dirty_pages);
		WRITE_ONCE(range->file->dirtied, jiffies);
		/* For the flusher's writepages: a write(2)'s pages tag nothing in the mapping. */
		__mark_inode_dirty(inode, I_DIRTY_PAGES);
	}
	return 0;
unreserve:
	if (fresh)
		fxfs_release_dirty(fs, inode->i_ino, (u64)fresh << PAGE_SHIFT);
	return err;
}

/*
 * fxdax: how many pages from `range`'s page `next`, the first after a write's iomap's, a PRIVATE
 * for the iomap makes private ahead of it (fxfs_dax_dirty): a write(2) following on from the file's
 * last, whose iomap reaches its end, `write_end`, takes as many as the file's run of such writes
 * says (fxfs_dax_write_follows), each mapped and written, in the range; none for a fault's, or a
 * zeroing's. A forked pgbench's log, a file of its base's, took a PRIVATE for every write, 62,146 a
 * minute, three pages each, each round trip on the way of the log's flush (TASK-164.10, GCE r649).
 */
static unsigned long fxfs_dax_write_ahead(struct inode *inode, struct fxfs_dax_range *range,
					  loff_t write_end, unsigned long next, bool not_write)
{
	unsigned long ahead = READ_ONCE(range->file->ahead), last;
	loff_t start = fxfs_dax_chunk_start(range);

	if (not_write || !ahead || start + ((loff_t)next << PAGE_SHIFT) < write_end ||
	    next >= range->pages)
		return 0;
	lockdep_assert_held_write(&inode->i_rwsem);
	last = min(range->pages, next + ahead);
	return fxfs_dax_pages_aheadable_run(range->state, next, last) - next;
}

/*
 * fxdax: maps a write of up to `length` bytes at `pos`, a fault's, a write(2)'s or zeroing's, at its
 * pages' own places in the window (doc-13), made dirty and private (fxfs_dax_dirty): no MAP, no
 * transaction, nothing moved. Returns holding the file's semaphore shared until
 * fxfs_dax_iomap_end: no writeback swaps the dirty set out, nor rebinding maps a page over, while
 * the store or the copy goes on. A slice's chunk, whose pages its region's other slices map, or a
 * range too small, is dropped (-EXDEV), and mapped again as a range of its own, as large as the
 * write needs. Zeroing leaves holes, and pages of zeros never written, be. A write past the file's
 * end grows it as it ends (fxfs_dax_iomap_end), unless it is a fault's.
 */
static int fxfs_dax_iomap_write(struct inode *inode, loff_t pos, loff_t length,
				unsigned int flags, struct iomap *iomap)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	bool fault = flags & IOMAP_FAULT, zero = flags & IOMAP_ZERO;
	loff_t start, end = min_t(loff_t, pos + length, round_up(pos + 1, FXFS_DAX_SIZE));
	unsigned long first, pages, page;
	struct fxfs_dax_range *range;
	int err;

	range = fxfs_dax_get_range(inode, pos >> FXFS_DAX_SHIFT, fault || zero ? 0 : end, fault,
				   false);
	if (IS_ERR(range))
		return PTR_ERR(range);
	start = fxfs_dax_chunk_start(range);
	if (range->region || start + ((loff_t)range->pages << PAGE_SHIFT) <= pos) {
		fxfs_dax_put_range(dax, range);
		return -EXDEV;
	}
	first = (pos - start) >> PAGE_SHIFT;
	end = min_t(loff_t, end, start + ((loff_t)min(range->pages, first + dax->iomap_pages) <<
					  PAGE_SHIFT));
	pages = DIV_ROUND_UP(end - start, PAGE_SIZE) - first;
	err = fxfs_dax_map_missing(inode, range, first, first + pages);
	if (err)
		goto put;
	if (zero) {
		bool hole = fxfs_dax_pages_unwritten(range->state, first);

		page = fxfs_dax_pages_unwritten_run(range->state, first, first + pages);
		pages = page - first;
		end = min_t(loff_t, end, start + ((loff_t)page << PAGE_SHIFT));
		if (hole) {
			up_read(&file->sem);
			fxfs_dax_put_range(dax, range);
			fxfs_dax_hole(iomap, end - pos);
			return 0;
		}
	}
	err = fxfs_dax_dirty(inode, range, first, pages, zero,
			     fxfs_dax_write_ahead(inode, range, pos + length, first + pages,
						  fault || zero));
	if (err)
		goto unlock;
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
unlock:
	up_read(&file->sem);
put:
	fxfs_dax_put_range(dax, range);
	return err;
}

/*
 * fxdax: for tests, as freezing does (fxfs_dax_frozen): writing to fxfs.dax_rebind_now asks the
 * budget's worker of the filesystem mounted last to write back, seal and rebind every private page
 * it can, and returns, the round queued. A check waits for it by the filesystem's stats: once
 * dax_rebinds_done reaches dax_rebinds_asked as read after the write, a round begun after the ask
 * has ended. Waited for here, the setter held kernel_param_lock (param_attr_store) through a
 * rebind a pause point held, and every later read of a parameter waited for it (GCE).
 */
static int fxfs_dax_rebind_now_set(const char *val, const struct kernel_param *kp)
{
	struct fxfs_dax *dax = READ_ONCE(fxfs_dax_stray_dax);

	if (!dax)
		return -EINVAL;
	atomic64_inc(&dax->rebinds_asked);
	fxfs_dax_kick(dax, true);
	return 0;
}

static const struct kernel_param_ops fxfs_dax_rebind_now_ops = { .set = fxfs_dax_rebind_now_set };
module_param_cb(dax_rebind_now, &fxfs_dax_rebind_now_ops, NULL, 0200);
MODULE_PARM_DESC(dax_rebind_now, "Have the worker write back, seal and rebind every private page, on the filesystem mounted last, not waited for: dax_rebinds_done reaches dax_rebinds_asked (tests)");

/* fxdax: fxfs.dax_rebind_every, set, starts the worker's rounds on the filesystem mounted last. */
static int fxfs_dax_rebind_every_set(const char *val, const struct kernel_param *kp)
{
	struct fxfs_dax *dax = READ_ONCE(fxfs_dax_stray_dax);
	int err = param_set_uint(val, kp);

	if (!err && dax && READ_ONCE(fxfs_dax_rebind_every))
		fxfs_dax_kick(dax, false);
	return err;
}

static const struct kernel_param_ops fxfs_dax_rebind_every_ops = {
	.set = fxfs_dax_rebind_every_set,
	.get = param_get_uint,
};
module_param_cb(dax_rebind_every, &fxfs_dax_rebind_every_ops, &fxfs_dax_rebind_every, 0644);
MODULE_PARM_DESC(dax_rebind_every, "Write back, seal and rebind every private page every this many ms (tests; 0: never)");

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
	u64 began;
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
		 * fxdax: its pages made private and dirty, nothing moved (doc-13). A write(2)'s timed,
		 * and no other iomap's: a read's two clock reads, for nothing, were 1-2% of a 91k-file
		 * tree's warm reread (GCE r901, r902, TASK-230).
		 */
		if (flags & (IOMAP_FAULT | IOMAP_ZERO))
			return fxfs_dax_iomap_write(inode, pos, length, flags, iomap);
		began = ktime_get_ns();
		err = fxfs_dax_iomap_write(inode, pos, length, flags, iomap);
		atomic64_add(ktime_get_ns() - began, &fxfs_dax_of(inode)->write_begin_ns);
		return err;
	}
	err = fxfs_dax_iomap_read(inode, pos, length, flags, iomap);
	/* fxdax: between the iomap and the read of what it maps, which its range keeps mapped. */
	if (!err) {
		fxfs_dax_pause_file(FXFS_DAX_STEP_READ_MAPPED, inode->i_ino);
		fxfs_dax_delay();
	}
	return err;
}

/*
 * A write's iomap held the file's semaphore shared, which goes now its store or copy is done
 * (fxfs_dax_iomap_write). A write(2) past the file's end grows it as it ends, by what it wrote, not
 * as it begins: a reader finding the file longer reads what the write wrote there, never the zeros
 * before it (TASK-133), as ext4's and XFS's DAX writes grow theirs.
 */
static int fxfs_dax_iomap_end(struct inode *inode, loff_t pos, loff_t length, ssize_t written,
			      unsigned int flags, struct iomap *iomap)
{
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	bool written_by_write = (flags & IOMAP_WRITE) && !(flags & (IOMAP_FAULT | IOMAP_ZERO));
	u64 began = written_by_write ? ktime_get_ns() : 0;

	if (!(flags & (IOMAP_WRITE | IOMAP_ZERO))) {
		if (iomap->private)
			fxfs_dax_put_range(dax, iomap->private);
		return 0;
	}
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

static const struct iomap_ops fxfs_dax_iomap_ops = {
	.iomap_begin = fxfs_dax_iomap_begin,
	.iomap_end = fxfs_dax_iomap_end,
};

static ssize_t fxfs_dax_read(struct kiocb *iocb, struct iov_iter *to)
{
	return dax_iomap_rw(iocb, to, &fxfs_dax_iomap_ops);
}

/*
 * fxdax: a read, whose copy takes no page fault: a read into a mapping of the file it reads holds
 * the range through its copy, which a fault on it may wait for, or the file's semaphore shared,
 * which a writeback waits behind. It faults the destination in with no iomap held, as a write
 * does its source.
 */
static ssize_t fxfs_dax_read_faulting_between(struct kiocb *iocb, struct iov_iter *to)
{
	struct inode *inode = file_inode(iocb->ki_filp);
	ssize_t ret, done = 0;
	unsigned int empty = 0;

	for (;;) {
		pagefault_disable();
		ret = fxfs_dax_read(iocb, to);
		pagefault_enable();
		if (ret > 0) {
			done += ret;
			empty = 0;
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
 * (fxfs_dax_write_iter). With page faults enabled, holding no iomap, so that a fault on a mapping
 * of the file itself gets what it needs. -EFAULT if the user's memory holds none of them; if it
 * holds fewer, those are what the write writes.
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
 * fxdax: where write_ns went (TASK-146.2), each part apart, so that they add up: the copy, which is
 * dax_iomap_rw less its iomaps', and what none of them holds, which a new step in the write would
 * show as.
 */
static void fxfs_dax_show_write_times(struct seq_file *m, struct fxfs_dax *dax)
{
	s64 begin = atomic64_read(&dax->write_begin_ns), end = atomic64_read(&dax->write_end_ns);
	s64 rw = atomic64_read(&dax->write_rw_ns), setup = atomic64_read(&dax->write_setup_ns);
	s64 fault_in = atomic64_read(&dax->write_fault_in_ns);
	s64 retry = atomic64_read(&dax->write_retry_ns), tail = atomic64_read(&dax->write_tail_ns);
	s64 lock = atomic64_read(&dax->write_lock_ns);
	s64 private = atomic64_read(&dax->write_private_ns);

	seq_printf(m, "dax_writes %lld\ndax_writes_ns %lld\ndax_write_private_ns %lld\n",
		   atomic64_read(&dax->writes), atomic64_read(&dax->write_ns), private);
	seq_printf(m, "dax_write_lock_ns %lld\n", lock);
	seq_printf(m, "dax_write_setup_ns %lld\ndax_write_fault_in_ns %lld\n", setup, fault_in);
	/* Read apart, as writes go on: a difference may come out below zero by a little. */
	seq_printf(m, "dax_write_begin_ns %lld\ndax_write_copy_ns %lld\ndax_write_end_ns %lld\n",
		   begin, max_t(s64, 0, rw - begin - end), end);
	seq_printf(m, "dax_write_retry_ns %lld\ndax_write_tail_ns %lld\n", retry, tail);
	seq_printf(m, "dax_write_other_ns %lld\n",
		   max_t(s64, 0, atomic64_read(&dax->write_ns) - private - lock - setup - fault_in -
			 rw - retry - tail));
}

/*
 * fxdax: whether a write(2) at `pos` follows on from `file`'s last, beginning where it ended or
 * within its last FXFS_DAX_FOLLOWS bytes, as a log's appends do (fxfs_dax_write_follows). Under the
 * inode's write lock as it is decided; read without it ahead of the lock, a guess
 * (fxfs_dax_private_before).
 */
static bool fxfs_dax_follows_on(struct fxfs_dax_file *file, loff_t pos)
{
	loff_t write_pos = READ_ONCE(file->write_pos), write_end = READ_ONCE(file->write_end);

	return write_end > 0 && pos >= max(write_pos, write_end - (loff_t)FXFS_DAX_FOLLOWS) &&
	       pos <= write_end;
}

/*
 * fxdax: the pages a PRIVATE makes private ahead of a write(2) of the bytes from `pos` to `end`
 * (fxfs_dax_write_ahead), as readahead's window grows: a write that follows on from the file's
 * last, beginning where it ended or within its last FXFS_DAX_FOLLOWS bytes, as a log's appends do,
 * rewriting the block they share, takes twice as many as the last, FXFS_DAX_AHEAD_FIRST at first,
 * FXFS_DAX_AHEAD_MOST at most; any other none, a rewrite of what the last wrote further back
 * among them. Under the inode's write lock (TASK-164.10).
 */
static void fxfs_dax_write_follows(struct fxfs_dax_file *file, loff_t pos, loff_t end)
{
	unsigned int ahead = 0;

	if (fxfs_dax_follows_on(file, pos))
		ahead = clamp(2 * file->ahead, FXFS_DAX_AHEAD_FIRST, FXFS_DAX_AHEAD_MOST);
	WRITE_ONCE(file->ahead, ahead);
	WRITE_ONCE(file->write_pos, pos);
	WRITE_ONCE(file->write_end, end);
}

/*
 * fxdax: makes the pages of `inode`'s chunk `chunk` from `pos` to `end` that a write(2) is about
 * to store to private, clean, as a PRIVATE ahead of a write does (fxfs_dax_dirty): those of a
 * range of the file's own already, mapped and written, if there is room for them below where the
 * budget's worker begins. Holding the inode's invalidate lock and the file's semaphore shared, as
 * a fault's iomap does, not the inode's lock (fxfs_dax_private_before). 0, or the PRIVATE's
 * failure, as fxfs_dax_dirty fails it: -ENOMEM if the guest had no memory for it, else -EIO.
 */
static int fxfs_dax_private_chunk_before(struct inode *inode, pgoff_t chunk, loff_t pos,
					 loff_t end)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range;
	unsigned long first, last;
	long newly, made;
	loff_t start;
	int err = 0;

	filemap_invalidate_lock_shared(inode->i_mapping);
	down_read(&file->sem);
	range = xa_load(&file->ranges, chunk);
	/* A slice's chunk is mapped again as a range of its own by the write's iomap. */
	if (!range || range->region)
		goto out;
	start = fxfs_dax_chunk_start(range);
	first = (pos - start) >> PAGE_SHIFT;
	last = min_t(unsigned long, range->pages, DIV_ROUND_UP(end - start, PAGE_SIZE));
	if (first >= last)
		goto out;
	last = fxfs_dax_pages_aheadable_run(range->state, first, last);
	if (first >= last)
		goto out;
	newly = fxfs_dax_pages_would_dirty(range->state, first, last - first).private;
	if (!newly || atomic_long_read(&dax->private) + newly >
			      dax->private_max - dax->private_max / 8 ||
	    !fxfs_dax_take_private(dax, newly))
		goto out;
	/* The pages private already, those others' iomaps made so meanwhile too, stay so. */
	err = fxfs_device_private(dax->dev, fxfs_dax_page_offset(range, first),
				  (u64)(last - first) << PAGE_SHIFT);
	atomic64_inc(&dax->privates);
	if (err) {
		err = fxfs_dax_private_failed(dax, fxfs_dax_page_offset(range, first), last - first,
					      newly, err);
		goto out;
	}
	made = fxfs_dax_pages_private_ahead(range->state, first, last - first);
	atomic64_add(made, &dax->private_before);
	/* Others' iomaps, holding the semaphore shared too, may have counted some meanwhile. */
	if (newly > made)
		atomic_long_sub(newly - made, &dax->private);
	atomic_long_add(made, &file->private_pages);
	WRITE_ONCE(file->dirtied, jiffies);
out:
	up_read(&file->sem);
	filemap_invalidate_unlock_shared(inode->i_mapping);
	return err;
}

/*
 * fxdax: makes the pages a write(2) of `count` bytes at `pos` stores to private before it takes the
 * inode's lock (fxfs_dax_private_chunk_before), as a fault makes its page private without it: a
 * write's PRIVATE, a round trip to the host of half a millisecond nested, held the lock, and the
 * file's other writes waited for it as long as the PRIVATEs took (TASK-164.11; a forked pgbench
 * on GCE, r802: 160 s of waits for 163 s of PRIVATEs). Its iomap, under the lock, finds them
 * private and asks for nothing. What it leaves, as a chunk not mapped yet, the file's end moving,
 * no room below the budget's worker, or rebinding meanwhile, the iomap makes private as before; a
 * PRIVATE failed fails the write, as the iomap's would (fxfs_dax_dirty). Not a write that follows
 * on from the file's last, a log's, whose iomap's PRIVATE takes the pages its next writes reach
 * too (fxfs_dax_write_ahead), which one made here would leave it none to ask.
 */
static int fxfs_dax_private_before(struct inode *inode, loff_t pos, size_t count)
{
	loff_t end = min_t(loff_t, pos + count, i_size_read(inode)), next;
	int err = 0;

	if (fxfs_dax_follows_on(fxfs_dax_file(inode), pos))
		return 0;
	for (; !err && pos < end; pos = next) {
		pgoff_t chunk = pos >> FXFS_DAX_SHIFT;

		next = min_t(loff_t, end, (loff_t)(chunk + 1) << FXFS_DAX_SHIFT);
		err = fxfs_dax_private_chunk_before(inode, chunk, pos, next);
	}
	return err;
}

/*
 * Writes through the window, copying with page faults disabled: the copy holds the file's
 * semaphore shared (fxfs_dax_iomap_write), and a fault on the user's memory, a mapping of the file
 * itself, would take it again, behind a writeback waiting for it. The memory is faulted in first,
 * and again whenever a copy stops short. fxdax: and bytes are taken in hand, the rest of the
 * chunk's, and copied from there, a copy that can't stop short, once a copy stopped short with
 * none copied (-EFAULT): what faulted in may be gone again. Then the write goes back to the user's
 * memory.
 */
static ssize_t fxfs_dax_write_iter(struct kiocb *iocb, struct iov_iter *from)
{
	struct inode *inode = file_inode(iocb->ki_filp);
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_source hand = {};
	struct iov_iter *source = from;
	loff_t old_size, end;
	u64 began, lap;
	ssize_t ret, done = 0;
	unsigned int drops = 0, throttles = 0;
	bool made_room = false;
	int err;

	if (iocb->ki_flags & IOCB_NOWAIT)
		return -EAGAIN;
	began = lap = ktime_get_ns();
	/* An append's place is the file's end, which only the lock holds still. */
	if (!(iocb->ki_flags & IOCB_APPEND)) {
		err = fxfs_dax_private_before(inode, iocb->ki_pos, iov_iter_count(from));
		if (err)
			return err;
	}
	fxfs_dax_lap(&dax->write_private_ns, &lap);
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
	end = iocb->ki_pos + iov_iter_count(from);
	fxfs_dax_write_follows(file, iocb->ki_pos, end);
	/* The ranges the file's end moves into, mapped as large as they need. */
	if (end > old_size) {
		WRITE_ONCE(file->growing, end);
		filemap_invalidate_lock(inode->i_mapping);
		ret = fxfs_dax_fit(inode, end);
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
		fxfs_dax_lap(&dax->write_rw_ns, &lap);
		if (ret > 0) {
			done += ret;
			drops = 0;
		} else if (ret == -ENOBUFS) {
			/*
			 * fxdax: no room among the private pages: waited for, holding nothing of the
			 * file's but the inode's lock (doc-13's budget).
			 */
			ret = fxfs_dax_throttle(dax, &throttles);
			if (ret)
				break;
		} else if (ret == -ENOSPC && !made_room) {
			/* fxdax: room made once, as the page cache's write_begin does. */
			fxfs_vfs_make_room(inode->i_sb);
			made_room = true;
		} else if (source == from && ret == -EFAULT) {
			ret = fxfs_dax_source_take(&hand, from, iocb->ki_pos);
			if (ret)
				break;
			source = &hand.iter;
		} else if (ret == -EXDEV) {
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
			/* A slice, or a range too small: dropped, the chunk is mapped again. */
			ret = fxfs_dax_drop_slice(inode, iocb->ki_pos, true);
			fxfs_dax_lap(&dax->write_retry_ns, &lap);
			if (ret)
				break;
		} else {
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
	 * A write that stopped short leaves the file no longer than it wrote: one that wrote nothing
	 * leaves it as it was, wherever it began past the end.
	 */
	if (iocb->ki_pos < end && end > old_size) {
		err = fxfs_data_resize(inode, done ? max(old_size, iocb->ki_pos) : old_size, NULL, NULL);
		if (err && !done)
			ret = err;
	}
	if (done)
		ret = done;
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
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct super_block *sb = inode->i_sb;
	u64 began = ktime_get_ns();
	unsigned int waits = 0, drops = 0, throttles = 0;
	bool made_room = false;
	vm_fault_t ret;
	unsigned long pfn;
	int error;

	trace_fxfs_dax_fault_start(inode->i_ino, vmf->pgoff, order, write);
	if (write) {
		sb_start_pagefault(sb);
		file_update_time(vmf->vma->vm_file);
	}
	for (;;) {
		long seen = atomic_long_read(&dax->released);

		/*
		 * fxdax: each try after the first yields first: retries that find what they wait for
		 * busy as soon as they look again kept the task off both of a 2-vCPU guest's CPUs,
		 * without preemption, for minutes (TASK-113).
		 */
		cond_resched();
		error = 0;
		/* fxdax: a follower's extents read and mapped under its fill barrier, as of one state. */
		fxfs_vfs_fill_begin(sb);
		filemap_invalidate_lock_shared(inode->i_mapping);
		ret = dax_iomap_fault(vmf, order, &pfn, &error, &fxfs_dax_iomap_ops);
		filemap_invalidate_unlock_shared(inode->i_mapping);
		fxfs_vfs_fill_end(sb);
		if (!(ret & VM_FAULT_ERROR))
			break;
		/*
		 * fxdax: no room among the private pages: waited for, holding nothing of the file's,
		 * as balance_dirty_pages after a fault (doc-13's budget).
		 */
		if (error == -ENOBUFS) {
			error = fxfs_dax_throttle(dax, &throttles);
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
		if (error == -ENOSPC && !made_room) {
			fxfs_vfs_make_room(sb);
			made_room = true;
			continue;
		}
		if (error != -EAGAIN && error != -EXDEV)
			break;
		/* fxdax: a fault killed meanwhile tries no more (TASK-79.49). */
		if (fatal_signal_pending(current)) {
			ret = VM_FAULT_SIGBUS;
			break;
		}
		if (error == -EXDEV) {
			/*
			 * A slice can't take the store: it maps the chunk no more. fxdax: a fault that
			 * can't get on fails, and says so, as a write does (TASK-140).
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
		fxfs_dax_make_room(dax, seen);
	}
	/* fxdax: a SIGBUS says nothing of why: the error here does (TASK-121). */
	if (ret & VM_FAULT_SIGBUS)
		pr_err_ratelimited("fxfs: a %s fault of order %u at page %lu of ino %lu failed: %d\n",
				   write ? "write" : "read", order, vmf->pgoff, inode->i_ino, error);
	if (write)
		sb_end_pagefault(sb);
	fxfs_hist_add(&dax->fault_hist, ktime_get_ns() - began);
	trace_fxfs_dax_fault_end(inode->i_ino, vmf->pgoff, ret);
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
 * falls back to pages, so that dirty pages are 4 KiB, and writeback write-protects them page by
 * page (wp_shared_mapping_range), which would leave a writable PMD writable.
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
	/*
	 * fxdax: read faults may map 2 MiB at once (fxfs_dax_huge_fault), as ext4's DAX does. And as
	 * ext4's, not VM_MIXEDMAP, which the folios dax.c maps need not be: a fork copies the PTEs of
	 * a mapping that is, a large folio's as one, all writable if any is (copy_present_ptes,
	 * FPB_MERGE_WRITE), which made writable, unfaulted, pages no PRIVATE had made the VM's: those
	 * of a chunk read in whole, a store having split its PMD. The child's store to one the host
	 * refused, again and again (GCE r861, TASK-149.16). A child's PTEs are its own faults'.
	 */
	vm_flags_set(vma, VM_HUGEPAGE);
	return 0;
}

/*
 * fxdax: the file's dirty pages written back (fxfs_dax_write_back), whatever `wbc` asks: the
 * flusher's, every 30 s or so, and fsync's and sync's (doc-13); then the kernel's dirty tags
 * cleaned, which say nothing to dax.c, so that the kernel takes the inode for clean
 * (fxfs_dax_clean_entries).
 */
static int fxfs_dax_writepages(struct address_space *mapping, struct writeback_control *wbc)
{
	struct inode *inode = mapping->host;
	int err, cleaned;

	err = fxfs_dax_write_back(inode);
	cleaned = fxfs_dax_clean_entries(inode, 0, LLONG_MAX);
	return err ?: cleaned;
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
	.fallocate = fxfs_data_fallocate,
	.remap_file_range = fxfs_data_remap_file_range,
};

/*
 * fxdax: once a seal ends: page-cache inodes evicted before it may be read in to DAX, which the
 * worker finds (fxfs_dax_prune).
 */
static void fxfs_dax_sealed(void *arg)
{
	struct fxfs_dax *dax = arg;

	/* fxdax: between the seal's end and what follows it (TASK-119.3). */
	fxfs_dax_delay();
	if (atomic_long_read(&dax->unsealed))
		fxfs_dax_kick(dax, false);
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

/* fxdax: the window's regions' space, the references held to them, and the slices that hold them. */
static void fxfs_dax_show_regions(struct fxfs_dax *dax, struct seq_file *m)
{
	unsigned long pages = 0, refs = 0, slices = 0;
	struct fxfs_dax_region *region;
	struct fxfs_dax_volume *volume;
	struct fxfs_dax_range *range;
	struct fxfs_dax_file *file;
	unsigned long ino, chunk;
	unsigned int bucket;

	spin_lock(&dax->regions_lock);
	hash_for_each(dax->regions, bucket, region, node) {
		pages += region->pages;
		refs += region->refs;
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
	seq_printf(m, "window_regions_bytes %lu\nwindow_regions_refs %lu\nwindow_slices %lu\n",
		   pages << PAGE_SHIFT, refs, slices);
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
	seq_printf(m, "window_unaligned %ld\n", atomic_long_read(&dax->unaligned));
	fxfs_dax_show_regions(dax, m);
	seq_printf(m, "window_lost_bytes %lld\n", atomic64_read(&dax->lost_bytes));
	seq_printf(m, "dax_huge_maps %lld\n", atomic64_read(&dax->huge_maps));
	seq_printf(m, "dax_extent_alloc_bytes %lld\n", atomic64_read(&dax->extent_alloc_bytes));
	/* fxdax: the private pages, writeback and rebinding (doc-13, TASK-164.2). */
	seq_printf(m, "dax_private_max %ld\n", dax->private_max);
	seq_printf(m, "dax_private_pages %ld\ndax_dirty_pages %ld\n",
		   atomic_long_read(&dax->private), atomic_long_read(&dax->dirty));
	seq_printf(m, "dax_writebacks %lld\ndax_written_pages %lld\n",
		   atomic64_read(&dax->writebacks), atomic64_read(&dax->written));
	seq_printf(m, "dax_rebound_pages %lld\ndax_throttled %lld\n",
		   atomic64_read(&dax->rebound), atomic64_read(&dax->throttled));
	seq_printf(m, "dax_rebinds_asked %lld\ndax_rebinds_done %lld\n",
		   atomic64_read(&dax->rebinds_asked), atomic64_read(&dax->rebinds_done));
	seq_printf(m, "dax_seals %lld\ndax_privates %lld\ndax_open_maps %lld\n",
		   atomic64_read(&dax->seals_made), atomic64_read(&dax->privates),
		   atomic64_read(&dax->open_maps));
	seq_printf(m, "dax_private_ahead_pages %lld\ndax_private_before_pages %lld\n",
		   atomic64_read(&dax->private_ahead), atomic64_read(&dax->private_before));
	seq_printf(m, "dax_unsealed_inodes %ld\n", atomic_long_read(&dax->unsealed));
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
	bool read_only;

	dax_dev = fxfs_device_dax(dev, &window_len);
	if (!dax_dev || window_len < FXFS_DAX_SIZE)
		return NULL;
	/* fxdax: a few iomaps' worth of private pages at least, but for a follower's (doc-13). */
	read_only = fxfs_device_read_only(dev);
	if (!read_only && fxfs_device_private_max(dev) < 8) {
		pr_warn("fxfs: the DAX window holds %llu private pages: no DAX\n",
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
	dax->read_only = read_only;
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
	mutex_init(&dax->files_lock);
	spin_lock_init(&dax->lock);
	INIT_LIST_HEAD(&dax->used);
	init_waitqueue_head(&dax->wait);
	atomic_long_set(&dax->released, 0);
	hash_init(dax->regions);
	spin_lock_init(&dax->regions_lock);
	dax->wq = alloc_workqueue("fxfs_dax", WQ_UNBOUND | WQ_MEM_RECLAIM, 0);
	if (!dax->wq) {
		fxfs_dax_destroy_pools(dax);
		kfree(dax);
		return ERR_PTR(-ENOMEM);
	}
	/* fxdax: the budget's worker, and the slice of the budget an iomap takes (doc-13). */
	INIT_WORK(&dax->work, fxfs_dax_work);
	init_waitqueue_head(&dax->budget_wait);
	dax->private_max = min_t(u64, fxfs_device_private_max(dev), LONG_MAX);
	dax->iomap_pages = clamp_t(unsigned long, dax->private_max / 8, 1, FXFS_DAX_PRIVATE_PAGES);
	fxfs_device_set_sealed(dev, fxfs_dax_sealed, dax);
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

	/* fxdax: the budget's worker, which uses the volume's files, waits. */
	fxfs_dax_stop(dax);
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
 * fxdax: before a sync seals what is written, every file's dirty pages written back, which a
 * write(2)'s leave the mapping untagged for, so that sync's writepages miss them (doc-13); and
 * those of a writeback under way committed, which fxfs_dax_write_back waits for: a file whose
 * pages the budget's writeback had swapped out had none dirty, and was sealed without them. Each
 * holding dirty or private pages, as a writeback's are until a rebind after its commit.
 */
int fxfs_dax_settle(struct fxfs_dax *dax)
{
	struct fxfs_dax_file **files;
	ssize_t nr, i;
	int err = 0;

	nr = fxfs_dax_busy_files(dax, &files);
	if (nr < 0)
		return nr;
	for (i = 0; i < nr; i++) {
		struct inode *inode = fxfs_dax_inode_of(dax, files[i]);

		if (inode) {
			int wrote = fxfs_dax_write_back(inode);

			err = err ?: wrote;
			iput(inode);
		}
		fxfs_dax_put_file(dax, files[i]);
	}
	kvfree(files);
	return err;
}

/*
 * fxdax: stops the budget's worker, and waits for it: before a volume goes, or the filesystem
 * unmounts, or freezes. A write it would have made room for waits meanwhile.
 */
void fxfs_dax_stop(struct fxfs_dax *dax)
{
	WRITE_ONCE(dax->stopping, true);
	cancel_work_sync(&dax->work);
}

/* fxdax: starts the budget's worker again, unless the filesystem is frozen, and has it run. */
void fxfs_dax_start(struct fxfs_dax *dax)
{
	if (READ_ONCE(dax->frozen))
		return;
	WRITE_ONCE(dax->stopping, false);
	fxfs_dax_kick(dax, false);
}

/*
 * fxdax: the filesystem frozen, durably, its writers stopped and their pages written back as the
 * VFS synced it: every private page is rebound, mapping its extent's page, sealed now, so that the
 * snapshot's window holds extents and zeros alone, as the device has it (doc-13). Those it can't,
 * as a writeback that found no memory leaves dirty (TASK-207) or a rebind refused, warned of: the
 * host refuses the snapshot's point while the window holds any (EBUSY, the extent daemon saying
 * how many: ExtentStore::point, TASK-203), and the VM goes on as it was.
 */
void fxfs_dax_frozen(struct fxfs_dax *dax)
{
	WRITE_ONCE(dax->frozen, true);
	fxfs_dax_relieve(dax, 0, false);
	if (atomic_long_read(&dax->private))
		pr_warn("fxfs: frozen with %ld private pages, %ld dirty: no snapshot\n",
			atomic_long_read(&dax->private), atomic_long_read(&dax->dirty));
}

/*
 * Stops the budget's worker, once nothing writes any more: before the filesystem unmounts, or
 * freezes, while its device takes requests.
 */
void fxfs_dax_quiesce(struct fxfs_dax *dax)
{
	fxfs_dax_stop(dax);
}

/* Starts the budget's worker again, as the filesystem thaws (fxfs_dax_quiesce). */
void fxfs_dax_thaw(struct fxfs_dax *dax)
{
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
	destroy_workqueue(dax->wq);
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
 * fxdax: whether the page-cache inode `ino`, of `volume`, has writes no barrier has sealed yet
 * (`unsealed`): read in now, the window would find their extents open, which it maps none of
 * (doc-13).
 */
static bool fxfs_dax_unsealed(struct fxfs_dax *dax, struct fxfs_dax_volume *volume, u64 ino)
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

	if (!dax || IS_DAX(inode))
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
	 * fxdax: an inode created in this mount is a page-cache inode while it is in memory, as one
	 * whose writes no barrier has sealed yet, read in again: S_DAX is set as an inode is read in,
	 * never on a live one, as ext4 and XFS have it (doc-13, TASK-164.3).
	 */
	if (created || fxfs_dax_unsealed(dax, fxfs_dax_volume_of(inode), inode->i_ino))
		return false;
	if (!READ_ONCE(fxfs_dax_fail_attach))
		file = fxfs_dax_attach(dax, inode);
	/* Without its file, it stays in the page cache. */
	if (!file)
		return false;
	fxfs_vfs_inode(inode)->dax.file = file;
	fxfs_vfs_inode(inode)->data = &fxfs_dax_data_ops;
	inode->i_flags |= S_DAX;
	inode->i_fop = &fxfs_dax_file_operations;
	inode->i_mapping->a_ops = &fxfs_dax_aops;
	return true;
}

/* fxdax: how often, and how far apart, an eviction tries again a writeback that found no memory. */
#define FXFS_DAX_EVICT_TRIES 100
#define FXFS_DAX_EVICT_WAIT_MS 10

/*
 * Detaches `inode`, which the kernel evicts, from its file, whose ranges stay mapped for the next
 * inode read in for it, its dirty pages written back first: unless no name links it, when they go.
 */
void fxfs_dax_evict(struct inode *inode)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range;
	unsigned long chunk;
	unsigned int tries;
	bool forgotten;
	int err;

	if (!dax || !IS_DAX(inode) || !file)
		return;
	if (inode->i_nlink) {
		/*
		 * fxdax: what Fxfs had no memory for stays dirty (TASK-207), which no writeback
		 * finds once the inode is gone: tried again, a while, then given up on, as an I/O
		 * error's are.
		 */
		err = fxfs_dax_write_back(inode);
		for (tries = 0; err == -ENOMEM && tries < FXFS_DAX_EVICT_TRIES; tries++) {
			msleep(FXFS_DAX_EVICT_WAIT_MS);
			err = fxfs_dax_write_back(inode);
		}
		if (err == -ENOMEM)
			err = fxfs_dax_write_back_keeping(inode, false);
		if (err)
			pr_warn("fxfs: writing back ino %lu as it is evicted: %d\n", inode->i_ino, err);
	} else {
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
 * that can't take them, to be mapped again as a range of its own. With the invalidate_lock held
 * for writing.
 */
static int fxfs_dax_zero_part(struct inode *inode, loff_t pos, loff_t len)
{
	unsigned int throttles = 0;
	bool zeroed;
	int err;

	for (;;) {
		err = dax_zero_range(inode, pos, len, &zeroed, &fxfs_dax_iomap_ops);
		/* fxdax: no room among the private pages: the worker makes some. */
		if (err == -ENOBUFS) {
			err = fxfs_dax_throttle(fxfs_dax_of(inode), &throttles);
			if (err)
				return err;
			continue;
		}
		if (err != -EXDEV)
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
 * written back and sealed, and its mapping dropped, which the copy replaces, or the window could
 * hide the copy until eviction. Fxfs's growths zero the block so: a truncate's, and a clone's
 * (TASK-153.4). With the invalidate_lock held for writing.
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
 * A reflink's `len` bytes at `pos` of `inode`, to or from them (fxfs_data_ops): their DAX entries
 * go, once their pages are unused, and the window's ranges of them, to be mapped again as they are
 * after the reflink, a range's dirty pages written back first (fxfs_dax_drop_pinned), as XFS writes
 * back both files' ranges: a destination's are other blocks. And the destination's ranges too
 * small for its new size, `size`, and its last block, which Fxfs zeroes past the end as it grows
 * (fxfs_dax_grow_begin).
 */
static int fxfs_dax_remap_begin(struct inode *inode, loff_t pos, loff_t len, loff_t size)
{
	int err;

	if (!fxfs_dax_file(inode) || !len)
		return 0;
	err = fxfs_dax_grow_begin(inode, i_size_read(inode), size);
	if (!err)
		err = fxfs_dax_break_layouts(inode, round_down(pos, PAGE_SIZE));
	if (!err)
		err = fxfs_dax_drop(inode, pos >> FXFS_DAX_SHIFT, (pos + len - 1) >> FXFS_DAX_SHIFT, -1);
	if (!err)
		err = fxfs_dax_fit(inode, size);
	return err;
}

/*
 * A hole punched from `start` to `end` (fxfs_data_ops): the pages' mappings go, once unused, from
 * the first, which its partial pages, zeroed through the window (fxfs_dax_zero_part), map again.
 */
static int fxfs_dax_punch_begin(struct inode *inode, loff_t start, loff_t end)
{
	return fxfs_dax_break_layouts(inode, round_down(start, PAGE_SIZE));
}

/*
 * The chunks of a hole's whole pages from `start` to `end`, before Fxfs frees their blocks: mapped
 * again from where their data is, if anywhere.
 */
static int fxfs_dax_punch_drop(struct inode *inode, loff_t start, loff_t end)
{
	return fxfs_dax_drop(inode, start >> FXFS_DAX_SHIFT, (end - 1) >> FXFS_DAX_SHIFT, -1);
}

/*
 * Before and after a change of size to `size` (fxfs_data_ops): the first waits for the pages past
 * the size to be unused, and zeroes the last page past it, holding the invalidate_lock and the
 * writeback lock, which the second lets go of, once it has mapped zeros past the size.
 */
static int fxfs_dax_resize_begin(struct inode *inode, loff_t size)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	int err;

	filemap_invalidate_lock(inode->i_mapping);
	/*
	 * An ordinary grow zeroes from Fxfs's end, which is as writeback leaves it: what is dirty is
	 * written first, so that it is the inode's (doc-13).
	 */
	err = fxfs_dax_write_back(inode);
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
	 * fxdax: no writeback until fxfs_dax_resize_end: one that read the size before the change
	 * wrote the pages past the new end after Fxfs cut them, and grew the file again. Taken last,
	 * as zeroing above may wait for the budget's worker, which writes back.
	 */
	mutex_lock(&file->wb_lock);
	return 0;
}

static void fxfs_dax_resize_end(struct inode *inode, loff_t size)
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
		if (!fxfs_dax_pages_any_mapped(range->state, page, range->pages))
			continue;
		/*
		 * fxdax: the pages past the end map zeros, which every page of a range maps
		 * (fxfs_dax_map_pages), the MAP freeing their private pages, their dirty pages' room
		 * released; unmapped if it fails, as a write maps them again.
		 */
		if (!range->region) {
			trace_fxfs_dax_map(FXFS_TRACE_MAP_ZEROS, inode->i_ino,
					   fxfs_dax_page_offset(range, page),
					   (u64)(range->pages - page) << PAGE_SHIFT, 1);
			err = fxfs_device_map_zeros(dax->dev, fxfs_dax_page_offset(range, page),
						    (u64)(range->pages - page) << PAGE_SHIFT);
			if (!err) {
				fxfs_dax_dropped(dax, range,
						 fxfs_dax_pages_cut(range->state, page,
								    range->pages - page), true);
				continue;
			}
			pr_warn("fxfs: mapping zeros past ino %lu's end at %llx of the DAX window failed: %d\n",
				inode->i_ino, fxfs_dax_page_offset(range, page), err);
		}
		/* A slice's pages past the end stay its region's: the slice no longer maps them. */
		err = range->region ? 0 :
		      fxfs_device_unmap(dax->dev, range->window_offset + ((u64)page << PAGE_SHIFT),
					(u64)(range->pages - page) << PAGE_SHIFT);
		if (err)
			pr_warn("fxfs: unmapping the DAX window at %llx failed: %d\n",
				range->window_offset, err);
		/* fxdax: the UNMAP freed their private pages, unless it failed. */
		fxfs_dax_dropped(dax, range,
				 fxfs_dax_pages_unmap(range->state, page, range->pages - page), !err);
	}
	up_write(&file->sem);
	mutex_unlock(&file->wb_lock);
	filemap_invalidate_unlock(inode->i_mapping);
}

/*
 * fxdax: what a DAX file's data's changes do in the window (data.c): a cut's tail zeroed through it
 * (fxfs_dax_resize_begin), and so a punch's partial pages (fxfs_dax_zero_part).
 */
static const struct fxfs_data_ops fxfs_dax_data_ops = {
	.resize_begin = fxfs_dax_resize_begin,
	.resize_end = fxfs_dax_resize_end,
	.keeps_tail = true,
	.punch_begin = fxfs_dax_punch_begin,
	.zero = fxfs_dax_zero_part,
	.punch_drop = fxfs_dax_punch_drop,
	.remap_begin = fxfs_dax_remap_begin,
};
