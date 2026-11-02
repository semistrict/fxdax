// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * A read fault of a PMD that says its pages are shared must find their folio's fields clean:
 * guest/tests' pmd_over_stale_index, step for step.
 *
 * dax_folio_put leaves an order 0 folio's index, which is its share count: a page that had an entry
 * at a nonzero index then looks shared, and dax_associate_entry doesn't make the folio compound for
 * a PMD (its WARN, fs/dax.c:458). An inode's first load gives a chunk's head page such an entry, in
 * the range's own space, and the eviction of the inode, whose ranges stay, ends the entry.
 */

#include "pause.h"

#define PIECE (1UL << 20)

static struct sim_mount *mount;
static struct file *file;
static char *data;

static void write_file(struct file *to, loff_t at, size_t len)
{
	ssize_t done = sim_write(to, at, data + at, len);

	check(done == len, "a write at %lld: %zd", at, done);
	check(!sim_fsync(to), "fsync");
}

static void scenario(void *arg)
{
	unsigned long addr, i;
	char got;
	int err;

	mount = sim_mount_new();
	file = sim_file_new(mount);
	data = kmalloc(4 * PIECE, GFP_KERNEL);
	for (i = 0; i < 4 * PIECE; i++)
		data[i] = i % 251 + 1;

	/* The second chunk in two extents, each whole: a range of its own, at a chunk's alignment. */
	write_file(file, 0, 2 * PIECE);
	write_file(file, 2 * PIECE, PIECE);
	write_file(file, 3 * PIECE, PIECE);

	/* Too small for a PMD: the head page's entry, at index 512, not shared. */
	addr = sim_mmap(file, 2 * PIECE, 2 * PAGE_SIZE);
	err = sim_user_load(addr, &got, 1);
	check(!err && got == data[2 * PIECE], "a load of page 0: %d, %#x", err, got);
	err = sim_user_load(addr + PAGE_SIZE, &got, 1);
	check(!err && got == data[2 * PIECE + PAGE_SIZE], "a load of page 1: %d, %#x", err, got);

	sim_munmap(addr);

	/* Eviction ends the entries, not the range. */
	check(!sim_fsync(file), "fsync");
	file = sim_file_read_in(mount, file);

	addr = sim_mmap(file, 2 * PIECE, 2 * PIECE);
	err = sim_user_load(addr, &got, 1);
	check(!err && got == data[2 * PIECE], "the load of a PMD: %d, %#x", err, got);
	check(sim_mm_is_huge(addr), "no PMD mapped the chunk");
	for (i = 0; i < 2 * PIECE; i += PAGE_SIZE) {
		err = sim_user_load(addr + i, &got, 1);
		check(!err && got == data[2 * PIECE + i], "byte %lu of the PMD: %d, %#x", i, err, got);
	}
	sim_munmap(addr);
}

int main(void)
{
	sim_init(1);
	sim_device_init(64UL << 20);
	sim_spawn("scenario", scenario, NULL);
	sim_run();
	sim_log("pmd_over_stale_index: passed\n");
	return 0;
}
