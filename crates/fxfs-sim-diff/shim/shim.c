// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * guest/sim's kernel, as much of it as its model of Fxfs (guest/sim/fxfs/fxfs.c) calls, for a
 * process of one thread (sim/sim.h): memory, locks that never wait, and the extent store under the
 * model as guest/sim's kernel/device.c has it: each extent as long as its furthest write, sealed by
 * a seal if a write reached it, written never after, and gone once freed. A write to a sealed
 * extent fails the run, as the host refuses it (Fxfs's fake device, SealedExtent).
 *
 * A failure, a sim_panic, is the harness's panic (fxfs_sim_diff_panic), which unwinds through the
 * model to the case that ran it, which proptest then shrinks: the model is left as it was, and
 * never used again.
 */

#include <stdarg.h>
#include <stdio.h>

#include <sim/host.h>

#include "../../../guest/linux/fs/fxfs/device.h"

/* The harness's (src/model.rs). */
__attribute__((noreturn)) void fxfs_sim_diff_panic(const char *message);
void fxfs_sim_diff_point(const char *what);

struct task_struct sim_task = { .comm = "harness" };

void sim_panic(const char *fmt, ...)
{
	char message[512];
	va_list args;

	va_start(args, fmt);
	vsnprintf(message, sizeof(message), fmt, args);
	va_end(args);
	fxfs_sim_diff_panic(message);
}

void sim_log(const char *fmt, ...)
{
	va_list args;

	va_start(args, fmt);
	vfprintf(stderr, fmt, args);
	va_end(args);
}

void *kzalloc(size_t size, gfp_t gfp)
{
	void *p = calloc(1, size ? size : 1);

	if (!p)
		sim_panic("out of memory");
	return p;
}

void *kmalloc(size_t size, gfp_t gfp)
{
	return kzalloc(size, gfp);
}

void *kcalloc(size_t n, size_t size, gfp_t gfp)
{
	return kzalloc(n * size, gfp);
}

void *krealloc(void *p, size_t size, gfp_t gfp)
{
	p = realloc(p, size ? size : 1);
	if (!p)
		sim_panic("out of memory");
	return p;
}

void kfree(const void *p)
{
	free((void *)p);
}

void mutex_init(struct mutex *lock)
{
	lock->held = false;
}

void mutex_lock(struct mutex *lock)
{
	if (lock->held)
		sim_panic("a mutex taken that is held: an operation run where it would wait");
	lock->held = true;
}

void mutex_unlock(struct mutex *lock)
{
	if (!lock->held)
		sim_panic("a mutex let go that isn't held");
	lock->held = false;
}

void init_rwsem(struct rw_semaphore *sem)
{
	sem->readers = 0;
	sem->writer = false;
}

void down_read(struct rw_semaphore *sem)
{
	if (sem->writer)
		sim_panic("a rwsem read that is written: an operation run where it would wait");
	sem->readers++;
}

void up_read(struct rw_semaphore *sem)
{
	if (!sem->readers)
		sim_panic("a rwsem's read let go that isn't held");
	sem->readers--;
}

void down_write(struct rw_semaphore *sem)
{
	if (sem->writer || sem->readers)
		sim_panic("a rwsem written that is held: an operation run where it would wait");
	sem->writer = true;
}

void up_write(struct rw_semaphore *sem)
{
	if (!sem->writer)
		sim_panic("a rwsem's write let go that isn't held");
	sem->writer = false;
}

void sim_point(const char *what)
{
	fxfs_sim_diff_point(what);
}

/* The model takes its own choices of the sim's scheduler: none that changes what it holds. */
u64 sim_random(void)
{
	static u64 state = 0x9e3779b97f4a7c15ULL;

	state ^= state << 13;
	state ^= state >> 7;
	state ^= state << 17;
	return state;
}

/* The store: extents laid out one after another, as kernel/device.c's, each its own memory. */
#define PMD_SIZE (2UL << 20)

struct extent {
	u64 sequence;
	u64 store_offset;
	u64 size;
	/* Pages it holds: as far as it was written. */
	u64 length;
	/* Whether a write reached it, and whether a seal found it so. */
	bool held;
	bool sealed;
	u8 *data;
};

static struct extent *extents;
static size_t nr_extents;
static u64 store_used;
static u64 device_seals;
static struct fxfs_device {
	int unused;
} device;

/* Forgets every extent: a new model's store. */
void sim_device_reset(void)
{
	size_t i;

	for (i = 0; i < nr_extents; i++)
		free(extents[i].data);
	free(extents);
	extents = NULL;
	nr_extents = 0;
	store_used = 0;
	device_seals = 0;
}

static struct extent *find_extent(u64 sequence)
{
	size_t i;

	for (i = 0; i < nr_extents; i++) {
		if (extents[i].sequence == sequence)
			return &extents[i];
	}
	sim_panic("the store has no extent %llu", (unsigned long long)sequence);
}

/* The extent the store's bytes `offset` to `offset + len` are of. */
static struct extent *extent_at(u64 offset, size_t len)
{
	size_t i;

	for (i = 0; i < nr_extents; i++) {
		struct extent *extent = &extents[i];

		if (offset >= extent->store_offset && offset + len <= extent->store_offset + extent->size)
			return extent;
	}
	sim_panic("the store's %zu bytes at %llu are of no extent", len, (unsigned long long)offset);
}

void sim_device_new_extent(u64 sequence, u64 size)
{
	extents = krealloc(extents, (nr_extents + 1) * sizeof(*extents), GFP_KERNEL);
	extents[nr_extents++] = (struct extent) {
		.sequence = sequence, .store_offset = store_used, .size = size,
		.data = kzalloc(size, GFP_KERNEL),
	};
	store_used += (size + PMD_SIZE - 1) / PMD_SIZE * PMD_SIZE;
}

u64 sim_device_store_offset(u64 sequence)
{
	return find_extent(sequence)->store_offset;
}

void sim_device_wrote(u64 sequence, u64 page, u64 pages)
{
	struct extent *extent = find_extent(sequence);

	if (extent->sealed)
		sim_panic("a write to extent %llu, which is sealed", (unsigned long long)sequence);
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
	size_t i;

	for (i = 0; i < nr_extents; i++)
		extents[i].sealed |= extents[i].held;
}

void sim_device_free_extent(u64 sequence)
{
	struct extent *extent = find_extent(sequence);

	free(extent->data);
	*extent = extents[--nr_extents];
}

void host_store_read(u64 offset, void *buf, size_t len)
{
	struct extent *extent = extent_at(offset, len);

	memcpy(buf, extent->data + (offset - extent->store_offset), len);
}

void host_store_write(u64 offset, const void *buf, size_t len)
{
	struct extent *extent = extent_at(offset, len);

	memcpy(extent->data + (offset - extent->store_offset), buf, len);
}

struct fxfs_device *fxfs_device_find(const char *tag, size_t len, struct fxfs_device_info *info)
{
	*info = (struct fxfs_device_info) {
		.block_size = PAGE_SIZE, .max_io = 1 << 20, .max_root = 4096, .branch = 1,
		.max_extent = 64 << 20,
	};
	return &device;
}

void fxfs_device_put(struct fxfs_device *dev)
{
}

/* The device counts a seal's beginning and its end. */
u64 fxfs_device_seals(struct fxfs_device *dev)
{
	return device_seals;
}

void fxfs_device_seal_begin(struct fxfs_device *dev)
{
	device_seals++;
}

void fxfs_device_seal_end(struct fxfs_device *dev)
{
	device_seals++;
}
