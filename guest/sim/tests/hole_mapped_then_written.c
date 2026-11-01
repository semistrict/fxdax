// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * A write over a hole a mapping maps zeros for must take those zeros away, whatever its pieces:
 * one task, no seals, no scheduling but what a single task does.
 *
 * Pages 0 and 2 are written, so that they take the staging extent's first two blocks, and page 1
 * is a hole, which a load maps the zero page for. A write of pages 0 and 1 leaves page 0 where it
 * is and moves page 1 to the extent's third block, so that the two aren't in a row in the window,
 * and the write takes two iomaps. The first, of page 0, says its pages are new (IOMAP_F_NEW); the
 * second finds page 1 writable already, as the first made it, and doesn't.
 */

#include <sim/sim.h>

#define check(cond, ...) do { if (!(cond)) sim_panic(__VA_ARGS__); } while (0)

static void scenario(void *arg)
{
	static char data[3 * PAGE_SIZE], loaded[PAGE_SIZE], written[2 * PAGE_SIZE];
	struct sim_mount *mount = sim_mount_new();
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

int main(void)
{
	sim_init(1);
	sim_device_init(32UL << 20);
	sim_spawn("scenario", scenario, NULL);
	sim_run();
	sim_log("hole_mapped_then_written: passed\n");
	return 0;
}
