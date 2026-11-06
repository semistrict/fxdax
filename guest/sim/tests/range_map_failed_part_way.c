// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * A range's MAPs failing part way leave the window mapping what the guest doesn't know of, which
 * its UNMAP must take all the same: one task, no seals.
 *
 * A file of a chunk, every other page of it written and synced apart, each an extent of its own,
 * holes between: a range of it maps more pieces than a MAP_MANY holds (fxfs_dax_device_map_many),
 * so in more requests than one. Cloned whole, and read in, a write to the clone makes its range: the first
 * request maps, the second fails, and the first map of the one by one after it fails too, which
 * fails the write; what the first request mapped, the guest counts mapped nowhere. Once the files
 * go and the filesystem unmounts, nothing in the window is mapped. Red with the range's UNMAP
 * sent only for the pages it says it mapped: none, its pages left mapped (failures, seed 972 at 200
 * operations, TASK-164.11).
 */

#include <sim/sim.h>
#include <linux/virtio_extent.h>

/* What it expects, void once a sweep's call has failed (kernel/sweep.c). */
#define check sim_expect

/* Pages written, every other one of a chunk's first: twice as many pieces, holes between. */
#define WRITTEN 200

static void scenario(void *arg)
{
	static char page[PAGE_SIZE];
	struct sim_mount *mount = sim_mount_new();
	struct file *file = sim_file_new(mount), *clone = sim_file_new(mount);
	ssize_t done;
	int err;
	u64 i;

	memset(page, 'a', sizeof(page));
	for (i = 0; i < WRITTEN; i++) {
		done = sim_write(file, 2 * i * PAGE_SIZE, page, sizeof(page));
		check(done == PAGE_SIZE, "writing page %llu: %zd", 2 * i, done);
		check(!sim_fsync(file), "the fsync of page %llu", 2 * i);
	}
	err = sim_clone(file, 0, clone, 0, 2 * WRITTEN * PAGE_SIZE);
	check(!err, "the clone: %d", err);
	/* Made here, it is a page-cache inode until read in again (doc-13, TASK-164.3). */
	clone = sim_file_read_in(mount, clone);
	/* The range's first MAP_MANY maps; its second fails, and then the first map alone. */
	sim_device_fail(VIRTIO_EXTENT_T_MAP, 1, 2);
	memset(page, 'b', sizeof(page));
	done = sim_write(clone, PAGE_SIZE, page, sizeof(page));
	check(done < 0, "a write whose range's MAPs failed: %zd", done);
	check(sim_device_failed() == 2, "%llu MAPs failed", sim_device_failed());
	sim_file_evict(clone);
	sim_file_evict(file);
	sim_mount_free(mount);
}

int main(void)
{
	sim_init(1);
	sim_device_init(32UL << 20);
	sim_spawn("scenario", scenario, NULL);
	sim_run();
	sim_end(SIM_END_READ_FILES);
	sim_log("range_map_failed_part_way: passed\n");
	return 0;
}
