// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * Writes of pages scattered over a file take no MAP each: they go through the staging extent's
 * region, and the pages lag, until a use of them maps them together. Whatever uses a lagging page
 * first finds what was written: a read, a load, a second write, a seal before them, a truncate
 * past it, or the inode read in again. A page lagging that anything took for what it held reads the
 * block it left.
 */

#include <sim/sim.h>

#define WINDOW_LEN (64UL << 20)
#define FILE_LEN (4UL << 20)
#define PAGES (FILE_LEN >> PAGE_SHIFT)
/* A page in three, in no order the file's blocks had. */
#define WRITES 256
#define STRIDE 677

#define check(cond, ...) do { if (!(cond)) sim_panic(__VA_ARGS__); } while (0)

static struct sim_mount *mount;
static struct file *file;
static char *expected, *got;

static unsigned long page_of(int write)
{
	return (unsigned long)write * STRIDE % PAGES;
}

static void write_page(unsigned long page, char byte)
{
	memset(expected + (page << PAGE_SHIFT), byte, PAGE_SIZE);
	check(sim_write(file, page << PAGE_SHIFT, expected + (page << PAGE_SHIFT), PAGE_SIZE) ==
	      PAGE_SIZE, "a write of page %lu", page);
}

static void check_file(const char *when, size_t len)
{
	size_t at;

	check(sim_read(file, 0, got, len) == len, "%s: a read", when);
	for (at = 0; at < len; at++)
		check(got[at] == expected[at], "%s: byte %zu is %#x, not %#x", when, at, got[at],
		      expected[at]);
}

/* Moves every page's block on the next write: the seal leaves them read-only where they are. */
static void seal(void)
{
	check(!sim_fsync(file), "fsync");
	/* The work after a seal, whose MAP of the next staging extent no count here is of. */
	msleep(10);
}

static void scattered(char byte)
{
	int write;

	for (write = 0; write < WRITES; write++)
		write_page(page_of(write), byte + write % 8);
}

static void scenario(void *arg)
{
	unsigned long addr, before, writes;
	char byte;
	int err;

	/* Scattered writes take few MAPs; the read after them, in chunks, a request for each. */
	seal();
	before = sim_device_map_requests();
	scattered(2);
	writes = sim_device_map_requests() - before;
	sim_log("writes_through: %d writes took %lu MAPs\n", WRITES, writes);
	check(writes < WRITES / 8, "%d writes took %lu MAPs", WRITES, writes);
	before = sim_device_map_requests();
	check_file("read after the writes", FILE_LEN);
	sim_log("writes_through: the read after them took %llu MAPs\n",
		sim_device_map_requests() - before);
	check(sim_device_map_requests() - before < WRITES / 8, "the read mapped them one by one");

	/* A load finds them, its PTE where the page always is. */
	seal();
	scattered(12);
	addr = sim_mmap(file, 0, FILE_LEN);
	err = sim_user_load(addr + (page_of(1) << PAGE_SHIFT), &byte, 1);
	check(!err && byte == expected[page_of(1) << PAGE_SHIFT], "a load: %d, %#x", err, byte);
	sim_munmap(addr);

	/* A second write of a page, whose first went through the region. */
	seal();
	write_page(5, 30);
	write_page(5, 31);
	check_file("read after a second write", FILE_LEN);

	/* A seal between the write and the read: the page's extent read-only by then. */
	seal();
	scattered(40);
	seal();
	check_file("read after a seal", FILE_LEN);

	/* A truncate past a lagging page, then its pages holes again. */
	seal();
	write_page(PAGES - 1, 50);
	write_page(PAGES / 2 + 1, 51);
	check(!sim_truncate(file, FILE_LEN / 2), "a truncate");
	memset(expected + FILE_LEN / 2, 0, FILE_LEN / 2);
	check(!sim_truncate(file, FILE_LEN), "growing again");
	check_file("read after a truncate", FILE_LEN);

	/* The inode gone: the range and what it lagged by stay. */
	seal();
	scattered(60);
	check(!sim_fsync(file), "fsync");
	file = sim_file_read_in(mount, file);
	check_file("read of the inode read in again", FILE_LEN);
}

int main(void)
{
	sim_init(1);
	sim_device_init(WINDOW_LEN);
	mount = sim_mount_new();
	expected = kmalloc(FILE_LEN, GFP_KERNEL);
	got = kmalloc(FILE_LEN, GFP_KERNEL);
	memset(expected, 1, FILE_LEN);
	file = sim_file_new(mount);
	/* A range of its own for each chunk, in extents of the file's own. */
	check(sim_write(file, 0, expected, FILE_LEN) == FILE_LEN, "the first write");
	sim_spawn("scenario", scenario, NULL);
	sim_run();
	sim_log("writes_through: passed\n");
	return 0;
}
