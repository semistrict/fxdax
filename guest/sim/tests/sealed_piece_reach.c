// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * A sealed piece of a writable region is never reached for outside the gate (TASK-44.21): one task.
 *
 * A clone has the end of an open extent read as sealed, and the file, cut short of it, has the rest,
 * open. The file's read maps that first, through the gate, which makes the region writable, as far
 * as it reaches. The clone then is all sealed pieces, of the same region, past what it maps, and is
 * read outside the gate: a MAP of them would be writable, in no gate. A clone of two pieces goes by
 * fxfs_dax_map_chunk, and of one by fxfs_dax_new_slice.
 */

#include "pause.h"

#define PIECE (1UL << 20)

static void reach(struct sim_mount *mount, char *data, bool pieces)
{
	struct file *file = sim_file_new(mount), *clone = sim_file_new(mount);
	struct file *keeper = sim_file_new(mount);
	ssize_t done;
	char got;
	int err;

	done = sim_write(file, 0, data, 2 * PIECE);
	check(done == 2 * PIECE, "the first chunk: %zd", done);
	check(!sim_fsync(file), "fsync");
	/* The second chunk in two extents, each whole: a range of its own, and no region of them. */
	done = sim_write(file, 2 * PIECE, data + 2 * PIECE, PIECE);
	check(done == PIECE, "its first extent: %zd", done);
	done = sim_write(file, 3 * PIECE, data + 3 * PIECE, PIECE);
	check(done == PIECE, "its second extent: %zd", done);

	err = sim_clone(file, 3 * PIECE - 2 * PAGE_SIZE, clone, 0, 2 * PAGE_SIZE);
	check(!err, "the clone: %d", err);
	if (pieces) {
		/* A piece with a hole before it: a chunk of two, not a slice. */
		err = sim_clone(file, 3 * PIECE - PAGE_SIZE, clone, 3 * PAGE_SIZE, PAGE_SIZE);
		check(!err, "the second clone: %d", err);
	}
	/* Shared still, once the file lets go of them. */
	err = sim_clone(file, 3 * PIECE - 2 * PAGE_SIZE, keeper, 0, 2 * PAGE_SIZE);
	check(!err, "the keeper's clone: %d", err);
	err = sim_truncate(file, 3 * PIECE - 2 * PAGE_SIZE);
	check(!err, "the truncate: %d", err);
	/* The file's open part maps first: its region writable, as far as it reaches. */
	done = sim_read(file, 2 * PIECE, &got, 1);
	check(done == 1 && got == data[2 * PIECE], "reading the file: %zd, %#x", done, got);
	done = sim_read(clone, 0, &got, 1);
	check(done == 1 && got == data[3 * PIECE - 2 * PAGE_SIZE], "reading the clone: %zd, %#x",
	      done, got);
}

static void scenario(void *arg)
{
	struct sim_mount *mount = sim_mount_new();
	char *data = kmalloc(4 * PIECE, GFP_KERNEL);
	unsigned long i;

	for (i = 0; i < 4 * PIECE; i++)
		data[i] = i % 251 + 1;
	reach(mount, data, true);
	reach(mount, data, false);
}

int main(void)
{
	sim_init(1);
	sim_device_init(64UL << 20);
	sim_spawn("scenario", scenario, NULL);
	sim_run();
	sim_log("sealed_piece_reach: passed\n");
	return 0;
}
