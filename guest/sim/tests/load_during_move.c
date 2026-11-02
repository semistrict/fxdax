// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * A load through a PTE of a page a fault is moving must read the page: guest/tests'
 * load_during_move, step for step.
 *
 * A store's fault moves its page's block; a seal ends before it reaches the gate, so that it goes
 * round to move the block again. Flushes meanwhile free the extent the block left, whose window page
 * maps zeros from then on, and another task's PTE of that page, read-only since the seal, reads
 * them.
 */

#include "pause.h"

#define SIZE (4UL << 20)

static struct sim_mount *mount;
static struct file *file, *other;
static unsigned long addr;
static char *data;
static bool loaded;
static char byte;

static void seal(void)
{
	ssize_t done = sim_write(other, 0, "sealed", 6);

	check(done == 6, "writing the other file: %zd", done);
	check(!sim_fsync(other), "a seal");
}

static void storer(void *arg)
{
	char stored = (char)0xee;
	int err = sim_user_store(addr + PAGE_SIZE + 100, &stored, 1);

	check(!err, "the store: %d", err);
}

static void loader(void *arg)
{
	int err = sim_user_load(addr + PAGE_SIZE, &byte, 1);

	check(!err, "the load: %d", err);
	loaded = true;
}

static void scenario(void *arg)
{
	unsigned long i;
	ssize_t done;
	char got;
	int err, waited;

	mount = sim_mount_new();
	file = sim_file_new(mount);
	other = sim_file_new(mount);
	data = kmalloc(SIZE, GFP_KERNEL);
	for (i = 0; i < SIZE; i++)
		data[i] = i % 251 + 1;
	done = sim_write(file, 0, data, SIZE);
	check(done == SIZE, "writing the file: %zd", done);
	check(!sim_fsync(file), "fsync");
	seal();
	addr = sim_mmap(file, 0, SIZE);
	err = sim_user_load(addr, &got, 1);
	check(!err && got == data[0], "the first load: %d, %#x", err, got);

	/* In an extent of its own, which goes as the page moves on. */
	err = sim_user_load(addr + PAGE_SIZE, &got, 1);
	check(!err && got == data[PAGE_SIZE], "a load: %d, %#x", err, got);
	err = sim_user_store(addr + PAGE_SIZE, &got, 1);
	check(!err, "a store: %d", err);
	seal();
	seal();
	/* A PTE of it, read-only since the seals. */
	err = sim_user_load(addr + PAGE_SIZE, &got, 1);
	check(!err && got == data[PAGE_SIZE], "the page before the fault: %d, %#x", err, got);

	pause_at(PAUSE_MOVED);
	sim_spawn("storer", storer, NULL);
	pause_wait(PAUSE_MOVED);
	/* A seal ends as the fault goes on to the gate, which it finds open, and it goes round. */
	seal();
	pause_at(PAUSE_RESEALED);
	pause_wait(PAUSE_RESEALED);
	for (i = 0; i < 4; i++)
		seal();
	/* The load, which with the page's PTE gone may wait for the fault, not the reverse. */
	sim_spawn("loader", loader, NULL);
	for (waited = 0; !loaded && waited < 1000; waited++)
		msleep(1);
	pause_at(0);
	while (!loaded)
		msleep(1);
	check(byte == data[PAGE_SIZE], "a load through a PTE during the move reads %#x, not %#x",
	      (u8)byte, (u8)data[PAGE_SIZE]);
}

int main(void)
{
	sim_init(1);
	sim_device_init(64UL << 20);
	sim_spawn("scenario", scenario, NULL);
	sim_run();
	sim_log("load_during_move: passed\n");
	return 0;
}
