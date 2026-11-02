// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * One task, with no VM: mounts a file on the DAX window, writes it, reads it back, takes faults
 * through a mapping, seals, and moves blocks through the staging extent.
 * Every step checks the bytes as read(2), the mapping and Fxfs's extents have them.
 */

#include <sim/sim.h>

#define WINDOW_LEN (64UL << 20)

static struct sim_mount *mount;
static struct file *file;

#define check(cond, ...) do { if (!(cond)) sim_panic(__VA_ARGS__); } while (0)

static void fill(char *buf, size_t len, unsigned char seed)
{
	size_t i;

	for (i = 0; i < len; i++)
		buf[i] = seed + i;
}

static void check_file(const char *what, size_t at, const char *expected, size_t len)
{
	char *got = kmalloc(len, GFP_KERNEL);
	struct inode *inode = file_inode(file);
	struct fxfs *fs = mount->info->fs;
	ssize_t read = sim_read(file, at, got, len);

	check(read == len, "%s: read %zd of %zu", what, read, len);
	check(!memcmp(got, expected, len), "%s: read(2) differs", what);
	fxfs_sim_read(fs, inode->i_ino, at, got, len);
	check(!memcmp(got, expected, len), "%s: Fxfs's extents differ", what);
	kfree(got);
}

static void scenario(void *arg)
{
	static char data[4 * PAGE_SIZE], back[4 * PAGE_SIZE];
	unsigned long addr;
	ssize_t done;
	int err;

	mount = sim_mount_new();
	file = sim_file_new(mount);

	/* A write through the window, of data in open extents, and a read of it. */
	fill(data, sizeof(data), 1);
	done = sim_write(file, 0, data, sizeof(data));
	check(done == sizeof(data), "write: %zd", done);
	done = sim_read(file, 0, back, sizeof(back));
	check(done == sizeof(back) && !memcmp(data, back, sizeof(back)), "read back: %zd", done);
	err = sim_fsync(file);
	check(!err, "fsync: %d", err);
	check_file("after a seal", 0, data, sizeof(data));

	/* A read fault, then a write fault: the page is sealed, so its block moves. */
	addr = sim_mmap(file, 0, sizeof(data));
	err = sim_user_load(addr + PAGE_SIZE + 5, back, 16);
	check(!err && !memcmp(back, data + PAGE_SIZE + 5, 16), "a load through a mapping: %d", err);
	memcpy(data + PAGE_SIZE + 100, "through a mapping", 17);
	err = sim_user_store(addr + PAGE_SIZE + 100, "through a mapping", 17);
	check(!err, "a store through a mapping: %d", err);
	check_file("after a store", 0, data, sizeof(data));

	/* A seal makes the mapping read-only, and the next store faults again. */
	err = sim_fsync(file);
	check(!err, "fsync: %d", err);
	check_file("after the second seal", 0, data, sizeof(data));
	memcpy(data + PAGE_SIZE + 200, "and again", 9);
	err = sim_user_store(addr + PAGE_SIZE + 200, "and again", 9);
	check(!err, "a store after a seal: %d", err);
	err = sim_fsync(file);
	check(!err, "fsync: %d", err);
	cond_resched();
	check_file("after the third seal", 0, data, sizeof(data));

	/* Past the end, then cut back: truncate unmaps what is past it. */
	err = sim_truncate(file, PAGE_SIZE + 10);
	check(!err, "truncate: %d", err);
	memset(data + PAGE_SIZE + 10, 0, sizeof(data) - PAGE_SIZE - 10);
	check_file("after a truncate", 0, data, PAGE_SIZE + 10);
	sim_munmap(addr);
	err = sim_fsync(file);
	check(!err, "fsync: %d", err);

	fxfs_dax_show_stats(mount->info->dax, NULL);
	sim_file_evict(file);
	sim_mount_free(mount);
}

int main(void)
{
	u64 moved, freed, opened, failed;

	sim_init(1);
	sim_device_init(WINDOW_LEN);
	sim_spawn("scenario", scenario, NULL);
	sim_run();
	fxfs_sim_stats(&moved, &freed, &opened, &failed);
	sim_log("smoke: passed: %llu points, %llu switches, %llu device maps, %llu blocks moved, %llu extents freed, trace %llx\n",
		sim_points(), sim_switches(), sim_device_maps(), moved, freed, sim_trace_hash());
	return 0;
}
