// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * The extent-store device under fs/fxfs/device.c (doc-4): the driver's interface, with the host's
 * rules for the window (crates/fxdax's map): a writable MAP is of an extent open or new, EPERM if
 * sealed; a read-only one, of an extent the store holds, ENOENT if not, and zeros past its end. A
 * seal makes writable pages read-only, and a freed extent's pages map zeros. The window is real
 * memory, so that a store the host would refuse faults, and fails the run (kernel/host.c).
 *
 * Requests complete in place, each a scheduling point: no queues, no interrupts, no async faults,
 * none of the host's own concurrency, and no latency but what the clock is told.
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
	WINDOW_WRITE,
};

struct extent {
	u64 sequence;
	u64 store_offset;
	u64 size;
	/* Pages it holds: as far as it was written, or mapped writably. */
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
	u64 maps;
} device;

static struct dax_device { int unused; } dax_device;
static int dax_readers;

void sim_device_init(u64 window_len)
{
	host_store_init(STORE_LEN);
	device.window = host_window_init(window_len);
	device.window_len = window_len;
	device.dax = &dax_device;
	device.pages = kcalloc(window_len >> PAGE_SHIFT, sizeof(*device.pages), GFP_KERNEL);
	INIT_LIST_HEAD(&device.extents);
	device.config = (struct vext_config) {
		.tag = "sim", .block_size = PAGE_SIZE, .map_alignment = PAGE_SIZE,
		.max_transfer = 1 << 20, .max_segments = 256, .max_root = 4096, .branch = 1,
		.max_extent = 64 << 20,
	};
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

void sim_device_seal(void)
{
	struct extent *extent;
	u64 page, pages = device.window_len >> PAGE_SHIFT;

	list_for_each_entry(extent, &device.extents, list)
		extent->sealed |= extent->held;
	for (page = 0; page < pages; page++) {
		if (device.pages[page].state != WINDOW_WRITE)
			continue;
		host_window_read_only(page << PAGE_SHIFT, PAGE_SIZE);
		device.pages[page].state = WINDOW_READ;
	}
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

void sim_device_describe(u64 page)
{
	static const char *const states[] = { "unmapped", "zeros", "read-only", "writable" };
	const struct window_page *at = &device.pages[page];

	sim_log("window page %llx: %s, extent %llu\n", page, states[at->state],
		at->extent ? at->extent->sequence : 0);
}

u64 sim_device_maps(void)
{
	return device.maps;
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
 * Fails the run if a page that stops being writable has a writable PTE: the host refuses the store
 * it may yet take, with no fault for the guest to wait in, whether or not one comes.
 */
static void check_no_writable_mapping(const char *what, u64 first, u64 pages)
{
	unsigned long addr = current->mm ? sim_mm_writable_mapping(first, pages) : 0;

	if (addr)
		sim_panic("%s of window pages %llx to %llx, which address %lx maps writably", what,
			  first, first + pages, addr);
}

static int map_one(u64 sequence, u64 offset, u64 len, u64 window_offset, bool writable)
{
	struct extent *extent = find_extent(sequence);
	u64 first = window_offset >> PAGE_SHIFT, page;
	u64 pages = len >> PAGE_SHIFT, in_extent = offset >> PAGE_SHIFT;
	u64 within;

	device.maps++;
	watch(writable ? "MAP writable" : "MAP read-only", first, pages, sequence, in_extent);
	if (window_offset + len > device.window_len)
		return -EINVAL;
	if (!extent || (!writable && !extent->held))
		return writable ? -EPERM : -ENOENT;
	if (writable) {
		if (extent->sealed)
			return -EPERM;
		if (offset + len > extent->size)
			return -ENOSPC;
		extent->held = true;
		extent->length = max(extent->length, in_extent + pages);
	}
	if (!writable)
		check_no_writable_mapping("a read-only MAP", first, pages);
	/* What of the range is past the extent's end, sealed, reads as zeros. */
	within = !extent->sealed ? pages :
		 in_extent >= extent->length ? 0 : min(pages, extent->length - in_extent);
	for (page = 0; page < pages; page++) {
		struct window_page *at = &device.pages[first + page];

		if (page < within) {
			at->extent = extent;
			at->state = writable ? WINDOW_WRITE : WINDOW_READ;
		} else {
			at->extent = NULL;
			at->state = WINDOW_ZEROS;
		}
	}
	if (within)
		host_window_map(window_offset, extent->store_offset + offset, within << PAGE_SHIFT,
				writable);
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
	for (page = first; page < first + (len >> PAGE_SHIFT); page++) {
		device.pages[page].extent = NULL;
		device.pages[page].state = WINDOW_NONE;
	}
	host_window_unmap(window_offset, len);
}

static int do_map(struct vext_request *req)
{
	const struct virtio_extent_map_entry *entries = req->data;
	size_t i, count = req->flags & VIRTIO_EXTENT_MAP_MANY ? req->data_len / sizeof(*entries) : 0;
	int err = map_one(req->id.sequence, req->offset, req->len, req->window_offset,
			  req->flags & VIRTIO_EXTENT_MAP_WRITE);

	for (i = 0; !err && i < count; i++)
		err = map_one(entries[i].map.id.sequence, entries[i].map.offset, entries[i].map.len,
			      entries[i].map.window_offset,
			      entries[i].flags & VIRTIO_EXTENT_MAP_WRITE);
	return err;
}

int virtio_extent_call(struct virtio_extent *vext, struct vext_request *req)
{
	int err;

	/* The request crosses to the host, and others run meanwhile; a seal may end on the way. */
	sim_point("a device request");
	switch (req->type) {
	case VIRTIO_EXTENT_T_MAP:
		err = do_map(req);
		break;
	case VIRTIO_EXTENT_T_UNMAP:
		unmap_one(req->window_offset, req->len);
		err = 0;
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
		host_store_read(extent->store_offset + req->offset, req->data, req->len);
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

void *virtio_extent_window(struct virtio_extent *vext, u64 *len)
{
	*len = vext->window_len;
	return vext->window;
}

struct dax_device *virtio_extent_dax(struct virtio_extent *vext)
{
	return vext->dax;
}

bool virtio_extent_dax_write(struct virtio_extent *vext)
{
	return true;
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
