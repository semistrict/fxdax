// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * Two mappings of a page, as two processes running one binary have, loading it in turn: once each
 * has faulted it in, neither faults again, whatever part of the window the page is in. A read fault
 * of a page its entry already names replaces nothing, so it must leave other mappings of it alone.
 */

#include <sim/sim.h>

#define WINDOW_LEN (8UL << 20)
#define HOT_PAGES 8
#define FILLERS 12
#define FILLER_LEN (2UL << 20)
#define TURNS 64

/* What it expects, void once a sweep's call has failed (kernel/sweep.c). */
#define check sim_expect

static struct sim_mount *mount;
static struct file *hot, *fillers[FILLERS];
static char data[FILLER_LEN];

static void write_all(struct file *to, size_t len)
{
	check(sim_write(to, 0, data, len) == len, "a write");
	check(!sim_fsync(to), "an fsync");
}

/* Reads every page of a file through a mapping, which the window's room is taken for. */
static void read_all(struct file *from, size_t len)
{
	unsigned long addr = sim_mmap(from, 0, len), at;
	char got;

	for (at = 0; at < len; at += PAGE_SIZE)
		check(!sim_user_load(addr + at, &got, 1), "a load at %lu", at);
	sim_munmap(addr);
}

static void turns(const char *what)
{
	unsigned long first = sim_mmap(hot, 0, HOT_PAGES << PAGE_SHIFT);
	unsigned long second = sim_mmap(hot, 0, HOT_PAGES << PAGE_SHIFT);
	unsigned long before = sim_mm_faults, faults;
	char got;
	int turn;

	for (turn = 0; turn < TURNS; turn++) {
		check(!sim_user_load(first, &got, 1), "a load of the first mapping");
		check(got == 1, "the first mapping reads %#x", got);
		check(!sim_user_load(second, &got, 1), "a load of the second mapping");
		check(got == 1, "the second mapping reads %#x", got);
	}
	faults = sim_mm_faults - before;
	sim_log("read_mappings_stay: %s: %lu faults in %d turns\n", what, faults, TURNS);
	check(faults == 2, "%s: %lu faults, not one for each mapping", what, faults);
	sim_munmap(second);
	sim_munmap(first);
}

static void scenario(void *arg)
{
	int i;

	turns("written");
	for (i = 0; i < FILLERS; i++)
		read_all(fillers[i], FILLER_LEN);
	turns("after reclaim");
}

int main(void)
{
	int i;

	sim_init(1);
	sim_device_init(WINDOW_LEN);
	mount = sim_mount_new();
	memset(data, 1, sizeof(data));
	hot = sim_file_new(mount);
	write_all(hot, HOT_PAGES << PAGE_SHIFT);
	for (i = 0; i < FILLERS; i++) {
		fillers[i] = sim_file_new(mount);
		write_all(fillers[i], FILLER_LEN);
	}
	sim_spawn("scenario", scenario, NULL);
	sim_run();
	sim_end(SIM_END_READ_FILES);
	sim_log("read_mappings_stay: passed\n");
	return 0;
}
