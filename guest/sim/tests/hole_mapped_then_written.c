// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * A write over a hole a mapping maps zeros for must take those zeros away, whatever its pieces:
 * one task, no seals, no scheduling but what a single task does.
 *
 * Pages 0 and 2 are written, and page 1 is a hole, which a load maps zeros for: the window's
 * (MAP_ZEROS, doc-13). A write of pages 0 and 1 makes both private, holding what they held, and
 * copies into them; no iomap says its pages are new, so the load's PTE stays, and reads the
 * write's bytes. On writable DAX, the write moved page 1 apart from page 0 in the window, and of
 * its two iomaps only the first said its pages were new: page 1's mapping stayed on the zero page.
 *
 * Then the one way a page in the file has no window page: a truncate's MAP of zeros past the new
 * end fails, which leaves the pages there unmapped (fxfs_dax_resize_end), and a write past them
 * grows the file over them. A load of one then is to map the window's page, as a write maps it,
 * not the kernel's zero page, which no write would take away (failures, seed 1669: a write from
 * the mapping copied its zeros over what read(2) read).
 */

#include <sim/sim.h>
#include <linux/virtio_extent.h>

#define check(cond, ...) do { if (!(cond)) sim_panic(__VA_ARGS__); } while (0)

static void written_over_a_hole(struct sim_mount *mount)
{
	static char data[3 * PAGE_SIZE], loaded[PAGE_SIZE], written[2 * PAGE_SIZE];
	struct file *file = sim_file_new(mount);
	unsigned long addr;
	ssize_t done;
	int err;

	memset(data, 'a', sizeof(data));
	done = sim_write(file, 0, data, PAGE_SIZE);
	check(done == PAGE_SIZE, "writing page 0: %zd", done);
	done = sim_write(file, 2 * PAGE_SIZE, data, PAGE_SIZE);
	check(done == PAGE_SIZE, "writing page 2: %zd", done);

	addr = sim_mmap(file, 0, sizeof(data));
	err = sim_user_load(addr + PAGE_SIZE, loaded, sizeof(loaded));
	check(!err && !loaded[0], "a load of the hole: %d, byte %#x", err, loaded[0]);

	memset(written, 'b', sizeof(written));
	done = sim_write(file, 0, written, sizeof(written));
	check(done == sizeof(written), "writing pages 0 and 1: %zd", done);

	done = sim_read(file, PAGE_SIZE, loaded, sizeof(loaded));
	check(done == sizeof(loaded) && loaded[0] == 'b', "read(2) of page 1: %zd, byte %#x", done,
	      loaded[0]);
	err = sim_user_load(addr + PAGE_SIZE, loaded, sizeof(loaded));
	check(!err, "a load of page 1: %d", err);
	check(loaded[0] == 'b', "a load of page 1 reads %#x where read(2) reads 'b'", loaded[0]);
}

static void written_over_a_failed_truncate(struct sim_mount *mount)
{
	static char data[4 * PAGE_SIZE], loaded[PAGE_SIZE], written[PAGE_SIZE];
	struct file *file = sim_file_new(mount);
	unsigned long addr;
	ssize_t done;
	int err;

	memset(data, 'a', sizeof(data));
	done = sim_write(file, 0, data, sizeof(data));
	check(done == sizeof(data), "writing pages 0 to 3: %zd", done);
	addr = sim_mmap(file, 0, sizeof(data));

	/* Pages 2 and 3, past the new end, are left with no window page. */
	sim_device_fail(VIRTIO_EXTENT_T_MAP, 0, 1);
	err = sim_truncate(file, 2 * PAGE_SIZE);
	check(!err, "a truncate whose MAP of zeros failed: %d", err);
	check(sim_device_failed() == 1, "no MAP of zeros failed");

	/* A write of page 3 grows the file over page 2, which nothing maps. */
	memset(written, 'c', sizeof(written));
	done = sim_write(file, 3 * PAGE_SIZE, written, sizeof(written));
	check(done == sizeof(written), "writing page 3: %zd", done);
	err = sim_user_load(addr + 2 * PAGE_SIZE, loaded, sizeof(loaded));
	check(!err && !loaded[0], "a load of page 2, cut and grown over: %d, byte %#x", err,
	      loaded[0]);

	memset(written, 'b', sizeof(written));
	done = sim_write(file, 2 * PAGE_SIZE, written, sizeof(written));
	check(done == sizeof(written), "writing page 2: %zd", done);
	done = sim_read(file, 2 * PAGE_SIZE, loaded, sizeof(loaded));
	check(done == sizeof(loaded) && loaded[0] == 'b', "read(2) of page 2: %zd, byte %#x", done,
	      loaded[0]);
	err = sim_user_load(addr + 2 * PAGE_SIZE, loaded, sizeof(loaded));
	check(!err, "a load of page 2: %d", err);
	check(loaded[0] == 'b', "a load of page 2 reads %#x where read(2) reads 'b'", loaded[0]);
}

static void scenario(void *arg)
{
	struct sim_mount *mount = sim_mount_new();

	written_over_a_hole(mount);
	written_over_a_failed_truncate(mount);
}

int main(void)
{
	sim_init(1);
	sim_device_init(32UL << 20);
	sim_spawn("scenario", scenario, NULL);
	sim_run();
	sim_log("hole_mapped_then_written: passed\n");
	return 0;
}
