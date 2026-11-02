// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * A region of an open extent is never read-only (TASK-140, TASK-153.3): one task, and no seal to end
 * what would go round.
 *
 * A clone has part of an open extent shared: a reader of the clone's own file mapped the extent's
 * region read-only, from a slice of its part of it, and now maps those blocks in a range's own
 * space. The file the extent is in, cut to a chunk of the extent alone, is one piece of it, open.
 * Had its region been made read-only, a slice of it would refuse a store, which Fxfs leaves where
 * it is: dropped, and made a slice again, for as long as no seal comes.
 */

#include "pause.h"

#define PIECE (1UL << 20)
#define HEAD (768UL << 10)

static void scenario(void *arg)
{
	struct sim_mount *mount = sim_mount_new();
	struct file *file = sim_file_new(mount), *clone = sim_file_new(mount);
	char *data = kmalloc(4 * PIECE, GFP_KERNEL);
	unsigned long addr, i;
	ssize_t done;
	char got;
	int err;

	for (i = 0; i < 4 * PIECE; i++)
		data[i] = i % 251 + 1;
	done = sim_write(file, 0, data, 2 * PIECE);
	check(done == 2 * PIECE, "the first chunk: %zd", done);
	check(!sim_fsync(file), "fsync");
	/* The second chunk in two extents, each whole: a range of its own, and no region of them. */
	done = sim_write(file, 2 * PIECE, data + 2 * PIECE, PIECE);
	check(done == PIECE, "its first extent: %zd", done);
	done = sim_write(file, 3 * PIECE, data + 3 * PIECE, PIECE);
	check(done == PIECE, "its second extent: %zd", done);

	/* All but the head of the first of them shared. */
	err = sim_clone(file, 2 * PIECE + HEAD, clone, 0, PIECE - HEAD);
	check(!err, "the clone: %d", err);
	done = sim_read(clone, 0, &got, 1);
	check(done == 1 && got == data[2 * PIECE + HEAD], "reading the clone: %zd, %#x", done, got);

	/* The chunk now the head alone: one piece, of an open extent. */
	err = sim_truncate(file, 2 * PIECE + HEAD);
	check(!err, "the truncate: %d", err);
	addr = sim_mmap(file, 2 * PIECE, PAGE_SIZE);
	got = 'x';
	err = sim_user_store(addr, &got, 1);
	check(!err, "a store to the head: %d", err);
	done = sim_read(file, 2 * PIECE, &got, 1);
	check(done == 1 && got == 'x', "reading the head: %zd, %#x", done, got);
}

int main(void)
{
	sim_init(1);
	sim_device_init(64UL << 20);
	sim_spawn("scenario", scenario, NULL);
	sim_run();
	sim_log("open_extent_read_only: passed\n");
	return 0;
}
