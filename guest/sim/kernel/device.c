// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * The extent-store device under fs/fxfs/device.c (doc-4): the driver's interface, with the host's
 * rules for the window (crates/fxdax's map), which copies on write (doc-13). A MAP is of an extent
 * the store holds, ENOENT if not, and maps zeros past its end; the window maps sealed extents and
 * zeros alone: a MAP of an open extent fails EPERM, which tests count, as Fxfs may take an extent
 * a seal forgot for sealed. MAP_ZEROS maps zeros. PRIVATE makes pages mapped read-only, a sealed
 * extent's or zeros, private, the VM's own, holding what they held, at the same address, until a
 * MAP or UNMAP over them frees them; ENOMEM past private_max. A freed extent's pages map zeros.
 *
 * The window is real memory, so that a store the host would refuse faults, and fails the run
 * (kernel/host.c). A store to a page mapped read-only, which the host would copy so, fails it too:
 * the guest makes every page private before a store can reach it (window_store).
 *
 * Requests complete in place, each a scheduling point: no queues, no interrupts, no async faults,
 * none of the host's own concurrency, and no latency but what the clock is told, or a MAP a test
 * holds back (sim_device_hold_map).
 */

#include <sim/sim.h>
#include <sim/host.h>
#include <linux/dax.h>
#include <linux/virtio_extent.h>

/* The store's memory, sparse: extents are laid out one after another, and never moved. */
#define STORE_LEN (1ULL << 40)
/* A device round trip, the most of a millisecond doc-4's MAP takes. */
#define REQUEST_NS 500000

enum window_state {
	WINDOW_NONE,
	WINDOW_ZEROS,
	WINDOW_READ,
	/* The VM's own page, which no extent names. */
	WINDOW_PRIVATE,
};

struct extent {
	u64 sequence;
	u64 store_offset;
	u64 size;
	/* Pages it holds: as far as it was written. */
	u64 length;
	/* Whether the device holds it: a writer has touched it. */
	bool held;
	/* Whether a seal found it held: one no request reached isn't sealed. */
	bool sealed;
	struct list_head list;
};

struct window_page {
	struct extent *extent;
	u8 state;
};

struct virtio_extent {
	struct vext_config config;
	struct dax_device *dax;
	u64 window_len;
	void *window;
	struct window_page *pages;
	struct list_head extents;
	u64 store_used;
	u64 maps, map_requests;
	/*
	 * The most private pages it holds, how many it holds, the most it held at once, how many MAPs
	 * and UNMAPs freed, how many PRIVATEs it refused for want of room, and how many maps of open
	 * extents it refused.
	 */
	u64 private_max;
	u64 private_pages;
	u64 private_most;
	u64 private_freed;
	u64 private_refused;
	u64 open_maps_refused;
} device;

static struct dax_device { int unused; } dax_device;
/* What a test has fail: how many of which requests, after how many more, or one in how many. */
static u32 failing_type;
static u64 failing_skip, failing, failed;
static unsigned int failing_odds;
static int dax_readers;

static const char *window_store(u64 offset);

void sim_device_init(u64 window_len)
{
	host_store_init(STORE_LEN);
	device.window = host_window_init(window_len, window_store);
	device.window_len = window_len;
	device.dax = &dax_device;
	device.pages = kcalloc(window_len >> PAGE_SHIFT, sizeof(*device.pages), GFP_KERNEL);
	INIT_LIST_HEAD(&device.extents);
	device.config = (struct vext_config) {
		.tag = "sim", .block_size = PAGE_SIZE, .map_alignment = PAGE_SIZE,
		.max_transfer = 1 << 20, .max_segments = 256, .max_root = 4096, .branch = 1,
		.max_extent = 64 << 20,
	};
	/* A quarter of the window's pages, the host's default (doc-13). */
	sim_device_private_max(window_len >> PAGE_SHIFT >> 2);
	sim_mm_init(device.window, window_len);
}

static struct extent *find_extent(u64 sequence)
{
	struct extent *extent;

	list_for_each_entry(extent, &device.extents, list) {
		if (extent->sequence == sequence)
			return extent;
	}
	return NULL;
}

void sim_device_new_extent(u64 sequence, u64 size)
{
	struct extent *extent = kzalloc(sizeof(*extent), GFP_KERNEL);

	extent->sequence = sequence;
	extent->size = size;
	extent->store_offset = device.store_used;
	device.store_used += ALIGN(size, PMD_SIZE);
	if (device.store_used > STORE_LEN)
		sim_panic("the store is out of space");
	list_add(&extent->list, &device.extents);
}

u64 sim_device_store_offset(u64 sequence)
{
	return find_extent(sequence)->store_offset;
}

/* Takes a write to `pages` pages of an open extent from `page`, which it then holds. */
void sim_device_wrote(u64 sequence, u64 page, u64 pages)
{
	struct extent *extent = find_extent(sequence);

	if (extent->sealed)
		sim_panic("a write to extent %llu, which is sealed", sequence);
	extent->held = true;
	extent->length = max(extent->length, page + pages);
}

bool sim_device_sealed(u64 sequence)
{
	return find_extent(sequence)->sealed;
}

u64 sim_device_length(u64 sequence)
{
	return find_extent(sequence)->length;
}

/* A COMMIT: it changes nothing in the window, which maps no open extent. */
void sim_device_seal(void)
{
	struct extent *extent;

	list_for_each_entry(extent, &device.extents, list)
		extent->sealed |= extent->held;
}

void sim_device_free_extent(u64 sequence)
{
	struct extent *extent = find_extent(sequence);
	u64 page, pages = device.window_len >> PAGE_SHIFT;

	for (page = 0; page < pages; page++) {
		if (device.pages[page].extent != extent)
			continue;
		host_window_map_zeros(page << PAGE_SHIFT, PAGE_SIZE);
		device.pages[page].extent = NULL;
		device.pages[page].state = WINDOW_ZEROS;
	}
	list_del(&extent->list);
	host_store_zero(extent->store_offset, ALIGN(extent->size, PMD_SIZE));
	kfree(extent);
}

/* The configuration's private_max, as the host sets it as it opens the device. */
void sim_device_private_max(u64 private_max)
{
	device.private_max = private_max;
	device.config.private_max = private_max;
}

u64 sim_device_private(void)
{
	return device.private_pages;
}

u64 sim_device_private_most(void)
{
	return device.private_most;
}

u64 sim_device_private_freed(void)
{
	return device.private_freed;
}

u64 sim_device_private_refused(void)
{
	return device.private_refused;
}

u64 sim_device_open_maps_refused(void)
{
	return device.open_maps_refused;
}

/* Makes window page `page` private: a copy of what it maps, or zeros, as the host's fault does. */
static void make_private(u64 page)
{
	struct window_page *at = &device.pages[page];

	host_window_private(page << PAGE_SHIFT, PAGE_SIZE, at->state == WINDOW_ZEROS);
	at->extent = NULL;
	at->state = WINDOW_PRIVATE;
	device.private_pages++;
	device.private_most = max(device.private_most, device.private_pages);
}

/* The private pages of `pages` from `first` go, which a MAP or UNMAP over them frees. */
static void free_private(u64 first, u64 pages)
{
	u64 page;

	for (page = first; page < first + pages; page++) {
		if (device.pages[page].state != WINDOW_PRIVATE)
			continue;
		device.private_pages--;
		device.private_freed++;
	}
}

/*
 * A store the window faulted on, at `offset`: why the host refuses it. The host would copy a page
 * mapped read-only, sealed or zeros, within private_max; but the guest makes each page
 * it dirties private first, by the PRIVATE of the iomap that dirties it (dax.c's
 * fxfs_dax_dirty, TASK-164.9), so that a store never reaches one: a store the host would copy
 * at is to a page no iomap made the VM's, whose block a writeback may move, and Fxfs free under it,
 * before the store lands (guest/sim's seed 2802). Refused, as the oracle of that rule: dirty
 * implies private. From the fault, in the store: no scheduling point, and no lock.
 */
static const char *window_store(u64 offset)
{
	const struct window_page *at = &device.pages[offset >> PAGE_SHIFT];

	if (at->state != WINDOW_READ && at->state != WINDOW_ZEROS)
		return "nothing mapped there";
	return "a store to a page no PRIVATE made the VM's: dirty without private";
}

void sim_device_describe(u64 page)
{
	static const char *const states[] = {
		"unmapped", "zeros", "read-only", "private",
	};
	const struct window_page *at = &device.pages[page];

	sim_log("window page %llx: %s, extent %llu\n", page, states[at->state],
		at->extent ? at->extent->sequence : 0);
}

u64 sim_device_maps(void)
{
	return device.maps;
}

u64 sim_device_map_requests(void)
{
	return device.map_requests;
}

/* The window page sim_device_watch watches, or -1. */
static u64 watched = ~0ULL;

void sim_device_watch(u64 page)
{
	watched = page;
}

static void watch(const char *what, u64 page, u64 pages, u64 sequence, u64 in_extent)
{
	if (watched >= page && watched < page + pages)
		sim_log("sim: window page %llx: %s by %s: extent %llu page %llu, at %llu ns\n", watched,
			what, current->comm, sequence, in_extent + watched - page, ktime_get_ns());
}

/*
 * Fails the run if a page a MAP or UNMAP replaces has a writable PTE: its private page goes, and
 * the host refuses the store it may yet take, with no fault for the guest to wait in, whether or
 * not one comes.
 */
static void check_no_writable_mapping(const char *what, u64 first, u64 pages)
{
	unsigned long addr = current->mm ? sim_mm_writable_mapping(first, pages) : 0;

	if (addr)
		sim_panic("%s of window pages %llx to %llx, which address %lx maps writably", what,
			  first, first + pages, addr);
}

/* MAP_ZEROS: zeros over `len` bytes at `window_offset`, whatever was there, private pages freed. */
static int map_zeros(u64 len, u64 window_offset)
{
	u64 first = window_offset >> PAGE_SHIFT, pages = len >> PAGE_SHIFT, page;

	device.maps++;
	watch("MAP zeros", first, pages, 0, 0);
	if (window_offset + len > device.window_len)
		return -EINVAL;
	check_no_writable_mapping("a MAP of zeros", first, pages);
	free_private(first, pages);
	for (page = first; page < first + pages; page++) {
		device.pages[page].extent = NULL;
		device.pages[page].state = WINDOW_ZEROS;
	}
	host_window_map_zeros(window_offset, len);
	return 0;
}

static int map_one(u64 sequence, u64 offset, u64 len, u64 window_offset)
{
	struct extent *extent = find_extent(sequence);
	u64 first = window_offset >> PAGE_SHIFT, page;
	u64 pages = len >> PAGE_SHIFT, in_extent = offset >> PAGE_SHIFT;
	u64 within;

	device.maps++;
	watch("MAP", first, pages, sequence, in_extent);
	if (window_offset + len > device.window_len)
		return -EINVAL;
	if (!extent || !extent->held)
		return -ENOENT;
	/* Doc-13's invariant 1: the window maps no open extent. */
	if (!extent->sealed) {
		device.open_maps_refused++;
		return -EPERM;
	}
	check_no_writable_mapping("a MAP", first, pages);
	free_private(first, pages);
	/* What of the range is past the extent's end reads as zeros. */
	within = in_extent >= extent->length ? 0 : min(pages, extent->length - in_extent);
	for (page = 0; page < pages; page++) {
		struct window_page *at = &device.pages[first + page];

		if (page < within) {
			at->extent = extent;
			at->state = WINDOW_READ;
		} else {
			at->extent = NULL;
			at->state = WINDOW_ZEROS;
		}
	}
	if (within)
		host_window_map(window_offset, extent->store_offset + offset, within << PAGE_SHIFT);
	if (within < pages)
		host_window_map_zeros(window_offset + (within << PAGE_SHIFT),
				      (pages - within) << PAGE_SHIFT);
	return 0;
}

static void unmap_one(u64 window_offset, u64 len)
{
	u64 page, first = window_offset >> PAGE_SHIFT;

	watch("UNMAP", first, len >> PAGE_SHIFT, 0, 0);
	check_no_writable_mapping("an UNMAP", first, len >> PAGE_SHIFT);
	free_private(first, len >> PAGE_SHIFT);
	for (page = first; page < first + (len >> PAGE_SHIFT); page++) {
		device.pages[page].extent = NULL;
		device.pages[page].state = WINDOW_NONE;
	}
	host_window_unmap(window_offset, len);
}

/* A MAP's map, or one of MAP_MANY's: of an extent, or, with MAP_ZEROS, of zeros. */
static int map_flags(u32 flags, u64 sequence, u64 offset, u64 len, u64 window_offset)
{
	if (flags & VIRTIO_EXTENT_MAP_ZEROS)
		return map_zeros(len, window_offset);
	return map_one(sequence, offset, len, window_offset);
}

static int do_map(struct vext_request *req)
{
	const struct virtio_extent_map_entry *entries = req->data;
	size_t i, count = req->flags & VIRTIO_EXTENT_MAP_MANY ? req->data_len / sizeof(*entries) : 0;
	int err = map_flags(req->flags, req->id.sequence, req->offset, req->len, req->window_offset);

	for (i = 0; !err && i < count; i++)
		err = map_flags(entries[i].flags, entries[i].map.id.sequence, entries[i].map.offset,
				entries[i].map.len, entries[i].map.window_offset);
	return err;
}

/*
 * PRIVATE: `len` bytes at `window_offset` private now, as stores would make them, all or none:
 * ENOMEM past private_max. Of pages mapped read-only alone, as a store to another is refused.
 */
static int do_private(u64 window_offset, u64 len)
{
	u64 first = window_offset >> PAGE_SHIFT, pages = len >> PAGE_SHIFT, page, more = 0;

	watch("PRIVATE", first, pages, 0, 0);
	if (!IS_ALIGNED(window_offset | len, PAGE_SIZE) || window_offset + len > device.window_len)
		return -EINVAL;
	for (page = first; page < first + pages; page++) {
		u8 state = device.pages[page].state;

		if (state != WINDOW_READ && state != WINDOW_ZEROS && state != WINDOW_PRIVATE)
			return -EINVAL;
		more += state != WINDOW_PRIVATE;
	}
	if (device.private_pages + more > device.private_max) {
		device.private_refused++;
		return -ENOMEM;
	}
	for (page = first; page < first + pages; page++) {
		if (device.pages[page].state != WINDOW_PRIVATE)
			make_private(page);
	}
	return 0;
}

/* Whether a test holds the next MAP back, and the pages of the one it holds, if any. */
static bool hold_next;
static u64 holding;

void sim_device_hold_map(void)
{
	hold_next = true;
}

u64 sim_device_held_map(void)
{
	return holding;
}

void sim_device_release_map(void)
{
	holding = 0;
}

/* Holds `req`, a MAP, on its way to the host, if a test asked, until the test releases it. */
static void hold_map(const struct vext_request *req)
{
	if (!hold_next)
		return;
	hold_next = false;
	holding = req->len >> PAGE_SHIFT;
	while (holding)
		msleep(1);
}

void sim_device_fail(u32 type, u64 skip, u64 count)
{
	failing_type = type;
	failing_skip = skip;
	failing = count;
}

void sim_device_fail_odds(unsigned int odds)
{
	failing_odds = odds;
}

u64 sim_device_failed(void)
{
	return failed;
}

/*
 * Window a failed UNMAP keeps from use for good (TASK-44.22), and how much of it may go to the odds:
 * past half of it, the ranges' half, nothing is left for a chunk, and requests fail for want of
 * room, whatever is under test, as 4 runs in 3,000 of `tests/failures` did.
 */
static u64 unmap_lost;
#define UNMAP_LOST_MAX (device.window_len / 4)

static bool request_fails(const struct vext_request *req)
{
	u32 type = req->type;
	bool fail = false;

	if (failing_odds && !(sim_random() % failing_odds)) {
		fail = type != VIRTIO_EXTENT_T_UNMAP || unmap_lost < UNMAP_LOST_MAX;
		unmap_lost += fail && type == VIRTIO_EXTENT_T_UNMAP ? req->len : 0;
	} else if (failing && type == failing_type && !failing_skip--) {
		failing_skip = 0;
		failing--;
		fail = true;
	}
	failed += fail;
	return fail;
}

int virtio_extent_call(struct virtio_extent *vext, struct vext_request *req)
{
	int err;

	/* The request crosses to the host, and others run meanwhile; a seal may end on the way. */
	sim_point("a device request");
	if ((req->type == VIRTIO_EXTENT_T_MAP || req->type == VIRTIO_EXTENT_T_UNMAP ||
	     req->type == VIRTIO_EXTENT_T_PRIVATE) && request_fails(req)) {
		sim_point("a device reply");
		return -EIO;
	}
	switch (req->type) {
	case VIRTIO_EXTENT_T_MAP:
		hold_map(req);
		device.map_requests++;
		err = do_map(req);
		break;
	case VIRTIO_EXTENT_T_UNMAP:
		unmap_one(req->window_offset, req->len);
		err = 0;
		break;
	case VIRTIO_EXTENT_T_PRIVATE:
		err = do_private(req->window_offset, req->len);
		break;
	default:
		sim_panic("request %u", req->type);
	}
	sim_point("a device reply");
	return err;
}

int virtio_extent_submit(struct virtio_extent *vext, struct vext_request *req)
{
	struct extent *extent = find_extent(req->id.sequence);

	sim_point("a device request");
	if (req->type != VIRTIO_EXTENT_T_READ)
		sim_panic("request %u", req->type);
	req->status = extent && extent->held ? 0 : -ENOENT;
	if (extent && extent->held)
		host_store_read(extent->store_offset + req->offset, req->data, req->data_len);
	req->done(req);
	return 0;
}

struct virtio_extent *virtio_extent_find(const char *tag)
{
	return &device;
}

void virtio_extent_put(struct virtio_extent *vext)
{
}

const struct vext_config *virtio_extent_config(struct virtio_extent *vext)
{
	return &vext->config;
}

struct dax_device *virtio_extent_dax(struct virtio_extent *vext, u64 *window_len)
{
	*window_len = vext->window_len;
	return vext->dax;
}

/* The most private pages the device holds at once. */
u64 virtio_extent_private_max(struct virtio_extent *vext)
{
	return vext->private_max;
}

bool virtio_extent_map_many(struct virtio_extent *vext)
{
	return true;
}

bool virtio_extent_map_ahead(struct virtio_extent *vext)
{
	return false;
}

bool virtio_extent_read_only(struct virtio_extent *vext)
{
	return false;
}

bool virtio_extent_write_commit(struct virtio_extent *vext)
{
	return false;
}

bool virtio_extent_freeze(struct virtio_extent *vext)
{
	return false;
}

bool virtio_extent_free_many(struct virtio_extent *vext)
{
	return false;
}

/* drivers/dax/super.c's, for the window. */

int dax_read_lock(void)
{
	dax_readers++;
	return 0;
}

void dax_read_unlock(int id)
{
	dax_readers--;
}

long dax_direct_access(struct dax_device *dax_dev, pgoff_t pgoff, long nr_pages,
		       enum dax_access_mode mode, void **kaddr, unsigned long *pfn)
{
	long pages = (device.window_len >> PAGE_SHIFT) - pgoff;

	WARN_ON(!dax_readers);
	if (kaddr)
		*kaddr = device.window + (pgoff << PAGE_SHIFT);
	if (pfn)
		*pfn = sim_window_pfn(pgoff);
	return min(nr_pages, pages);
}

static size_t iter_copy(struct iov_iter *iter, void *kaddr, size_t bytes, bool to_iter)
{
	size_t done;

	if (iter->user) {
		done = sim_copy_user(iter->uaddr, kaddr, bytes, to_iter);
		iter->uaddr += done;
	} else {
		if (to_iter)
			memcpy(iter->kbuf, kaddr, bytes);
		else
			memcpy(kaddr, iter->kbuf, bytes);
		iter->kbuf += bytes;
		done = bytes;
	}
	iter->count -= done;
	return done;
}

size_t dax_copy_from_iter(struct dax_device *dax_dev, pgoff_t pgoff, void *addr, size_t bytes,
			  struct iov_iter *i)
{
	return iter_copy(i, addr, min(bytes, i->count), false);
}

size_t dax_copy_to_iter(struct dax_device *dax_dev, pgoff_t pgoff, void *addr, size_t bytes,
			struct iov_iter *i)
{
	return iter_copy(i, addr, min(bytes, i->count), true);
}

int dax_zero_page_range(struct dax_device *dax_dev, pgoff_t pgoff, size_t nr_pages)
{
	memset(device.window + (pgoff << PAGE_SHIFT), 0, nr_pages << PAGE_SHIFT);
	return 0;
}

void dax_flush(struct dax_device *dax_dev, void *addr, size_t size)
{
}
