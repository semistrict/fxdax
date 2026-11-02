// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * Tasks reading through mappings of more than the window holds, as a build's typecheck does: many
 * small files of one staging extent and a few large ones, written and sealed first, then mapped by
 * every reader at once while the window's ranges and regions are reclaimed under them.
 *
 *   read_pressure SEED [READERS] [ROUNDS]
 */

#include <sim/sim.h>

#define WINDOW_LEN (8UL << 20)
#define FILES 64
#define POPULAR 8
#define MAX_READERS 16

#define check(cond, ...) do { if (!(cond)) sim_panic(__VA_ARGS__); } while (0)

static struct sim_mount *mount;
static struct file *files[FILES];
static size_t sizes[FILES];
static int readers = 6;
static unsigned long rounds = 40;
static bool written;
static wait_queue_head_t ready;

static char byte_of(int file, unsigned long page)
{
	return (file * 31 + page * 7) % 255 + 1;
}

static void write_file(int n)
{
	static char buf[64 * PAGE_SIZE];
	unsigned long pages = sizes[n] >> PAGE_SHIFT, page, i, run;

	for (page = 0; page < pages; page += run) {
		run = min(pages - page, 64UL);
		for (i = 0; i < run; i++)
			memset(buf + (i << PAGE_SHIFT), byte_of(n, page + i), PAGE_SIZE);
		check(sim_write(files[n], (loff_t)page << PAGE_SHIFT, buf,
				run << PAGE_SHIFT) == run << PAGE_SHIFT,
		      "a write of page %lu of file %d", page, n);
	}
}

static void writer_task(void *arg)
{
	int n;

	for (n = 0; n < FILES; n++) {
		write_file(n);
		if (n % 8 == 7)
			check(!sim_fsync(files[n]), "an fsync");
	}
	check(!sim_fsync(files[0]), "an fsync");
	WRITE_ONCE(written, true);
	wake_up_all(&ready);
}

static void reader_task(void *arg)
{
	unsigned long round;

	wait_event(ready, READ_ONCE(written));
	for (round = 0; round < rounds; round++) {
		int n = sim_random() % 2 ? sim_random() % POPULAR : sim_random() % FILES;
		unsigned long pages = sizes[n] >> PAGE_SHIFT, i, touches;
		unsigned long addr = sim_mmap(files[n], 0, sizes[n]);

		touches = min(pages, 32UL);
		for (i = 0; i < touches; i++) {
			unsigned long page = pages <= 32 ? i : sim_random() % pages;
			char got;
			int err = sim_user_load(addr + (page << PAGE_SHIFT), &got, 1);

			if (err && sim_warns_continue)
				continue;
			check(!err, "a load of page %lu of file %d: %d", page, n, err);
			check(got == byte_of(n, page), "page %lu of file %d reads %#x", page, n, got);
		}
		sim_munmap(addr);
	}
}

/* Seals go on through the reads. */
static void syncer_task(void *arg)
{
	struct file *file;
	static char buf[PAGE_SIZE];
	int i;

	wait_event(ready, READ_ONCE(written));
	file = sim_file_new(mount);
	for (i = 0; i < 24; i++) {
		check(sim_write(file, (loff_t)(i % 8) << PAGE_SHIFT, buf, PAGE_SIZE) == PAGE_SIZE,
		      "a write");
		check(!sim_fsync(file), "an fsync");
	}
}

int main(int argc, char **argv)
{
	unsigned long long seed = 1;
	int n;

	if (argc > 1)
		sscanf(argv[1], "%llu", &seed);
	if (argc > 2)
		sscanf(argv[2], "%d", &readers);
	if (argc > 3)
		sscanf(argv[3], "%lu", &rounds);
	if (argc > 4)
		sim_warns_continue = true;
	sim_init(seed);
	sim_device_init(WINDOW_LEN);
	init_waitqueue_head(&ready);
	mount = sim_mount_new();
	for (n = 0; n < FILES; n++) {
		unsigned long kind = sim_random() % 8, pages;

		pages = kind == 0 ? 512 + sim_random() % 256 : kind == 1 ? 64 + sim_random() % 448 :
			1 + sim_random() % 16;
		/* Those all read are large enough to fill chunks, some of them PMDs. */
		if (n < POPULAR && n % 2)
			pages = 512 * (1 + n / 2);
		files[n] = sim_file_new(mount);
		sizes[n] = pages << PAGE_SHIFT;
	}
	sim_spawn("writer", writer_task, NULL);
	sim_spawn("syncer", syncer_task, NULL);
	for (n = 0; n < readers; n++)
		sim_spawn("reader", reader_task, NULL);
	sim_run();
	sim_log("read_pressure: seed %llu passed: %llu points, %llu switches, %llu device maps\n",
		seed, sim_points(), sim_switches(), sim_device_maps());
	return 0;
}
