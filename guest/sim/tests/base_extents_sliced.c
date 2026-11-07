// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * Small files in one sealed extent whose size Fxfs doesn't count, as a volume over a base counts
 * none of the base's (an attached generation's, whose filesystem's allocator counts them), read as
 * slices of the extent's region, which one MAP maps: the device maps zeros past an extent's end, so
 * a region needn't know where it ends. Red with a slice refused for an extent of no size, each file
 * a range of its own, a MAP each: a sandbox's cold read of the Linux tree asked 91,072 MAPs, 19.6 s
 * of its 27 s, against a fresh VM's 1,005 on the publisher's store, 5.7 s (GCE r940, TASK-229).
 */

#include <sim/sim.h>

/* What it expects, void once a sweep's call has failed (kernel/sweep.c). */
#define check sim_expect

/* Small files, a page each: one staging extent holds them all. */
#define FILES 8

/* Drops `file`'s ranges, whatever they map. */
static void drop_ranges(struct sim_mount *mount, struct file *file)
{
	struct inode *inode = file_inode(file);
	int err;

	filemap_invalidate_lock(inode->i_mapping);
	err = fxfs_dax_changed(mount->info->dax, mount->info->dax_volume, inode, inode->i_ino, 0,
			       MAX_LFS_FILESIZE);
	filemap_invalidate_unlock(inode->i_mapping);
	check(!err, "dropping ino %lu's ranges: %d", inode->i_ino, err);
}

static void scenario(void *arg)
{
	static char page[PAGE_SIZE], read[PAGE_SIZE];
	struct sim_mount *mount = sim_mount_new();
	struct file *files[FILES];
	u64 requests, slices;
	ssize_t done;
	int f;

	for (f = 0; f < FILES; f++) {
		files[f] = sim_file_new(mount);
		memset(page, 'a' + f, sizeof(page));
		done = sim_write(files[f], 0, page, sizeof(page));
		check(done == PAGE_SIZE, "writing file %d: %zd", f, done);
	}
	/* Written back, each after the last in the staging extent, and then sealed at once. */
	for (f = 0; f < FILES; f++)
		check(!fxfs_dax_write_back(file_inode(files[f])), "file %d's writeback", f);
	check(!sim_fsync(files[FILES - 1]), "the fsync");
	fxfs_sim_extent_sizes_unknown(true);
	/*
	 * Made here, each is a page-cache inode until read in again (doc-13, TASK-164.3), and its
	 * ranges, which outlive its inode, dropped: as a VM that attached the base finds them, none.
	 */
	for (f = 0; f < FILES; f++) {
		files[f] = sim_file_read_in(mount, files[f]);
		drop_ranges(mount, files[f]);
	}
	requests = sim_device_map_requests();
	slices = sim_dax_stat(mount, "window_slices");
	for (f = 0; f < FILES; f++) {
		done = sim_read(files[f], 0, read, sizeof(read));
		check(done == PAGE_SIZE, "reading file %d: %zd", f, done);
		memset(page, 'a' + f, sizeof(page));
		check(!memcmp(read, page, sizeof(page)), "file %d read wrong", f);
	}
	check(sim_dax_stat(mount, "window_slices") - slices == FILES, "%llu of %d files sliced",
	      sim_dax_stat(mount, "window_slices") - slices, FILES);
	check(sim_device_map_requests() - requests == 1, "%llu MAP requests for %d files",
	      sim_device_map_requests() - requests, FILES);
	fxfs_sim_extent_sizes_unknown(false);
	for (f = 0; f < FILES; f++)
		sim_file_evict(files[f]);
	sim_mount_free(mount);
}

int main(void)
{
	sim_init(1);
	sim_device_init(32UL << 20);
	sim_spawn("scenario", scenario, NULL);
	sim_run();
	sim_end(SIM_END_READ_FILES);
	sim_log("base_extents_sliced: passed\n");
	return 0;
}
