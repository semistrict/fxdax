// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * A write that says its pages are new must not put a PMD's folio that a mapping still maps:
 * guest/tests' pmd_entry_invalidated_mapped, step for step (TASK-150.21).
 *
 * A load maps a chunk with a PMD. A store's fault moves page 0's block, the page holding zeros, so
 * that nothing is written to where it went; a seal shuts the gate before it gets on, and its task
 * is killed, so that it leaves the page where it moved, in an extent the device doesn't hold: a
 * hole. The PMD's entry stays, and a load of page 1 maps a PTE of its folio. A write(2) of page 0
 * then says its page is new, and invalidate_inode_pages2_range put the PMD's folio, splitting it
 * under that PTE (fs/dax.c's WARN in dax_folio_put), before it unmapped the range.
 */

#include "pause.h"
#include <linux/virtio_extent.h>

#define SIZE (4UL << 20)
#define FIRST (1UL << 20)
#define PAUSE_SEALING (1UL << 1)

static struct sim_mount *mount;
static struct file *file, *other;
static unsigned long addr;
static char *data;

static void storer(void *arg)
{
	char stored = 'x';

	/* Killed, it gives up: the fault fails. */
	sim_user_store(addr + 100, &stored, 1);
}

static void sealer(void *arg)
{
	check(!sim_fsync(other), "the seal");
}

/* Waits for the staging extent's MAP to fail. */
static void pause_wait_failed(void)
{
	int waited;

	for (waited = 0; !sim_device_failed(); waited++) {
		check(waited < 10000, "no MAP failed");
		msleep(1);
	}
}

static void wait_for(struct task_struct *task)
{
	while (!task->done)
		msleep(1);
}

static void scenario(void *arg)
{
	struct task_struct *store, *seal;
	char written[PAGE_SIZE], got[PAGE_SIZE];
	unsigned long i;
	ssize_t done;
	int err;

	mount = sim_mount_new();
	file = sim_file_new(mount);
	other = sim_file_new(mount);
	data = kmalloc(SIZE, GFP_KERNEL);
	/* Page 0 zeros, which a move keeps without writing them to the device. */
	for (i = 0; i < SIZE; i++)
		data[i] = i < PAGE_SIZE ? 0 : i % 251 + 1;
	/* In two extents, which only a range of its own maps, not a slice of a region. */
	sim_param_set("dax_writes_through", "N");
	done = sim_write(file, 0, data, FIRST);
	check(done == FIRST, "writing the file: %zd", done);
	done = sim_write(file, FIRST, data + FIRST, SIZE - FIRST);
	check(done == SIZE - FIRST, "writing the rest of it: %zd", done);
	check(!sim_fsync(file), "fsync");
	addr = sim_mmap(file, 0, SIZE);
	err = sim_user_load(addr + PAGE_SIZE, got, 1);
	check(!err && got[0] == data[PAGE_SIZE], "the load of a PMD: %d, %#x", err, got[0]);
	check(sim_mm_is_huge(addr), "no PMD mapped the chunk");

	/*
	 * A seal, after a write that took blocks of the staging extent, which it replaces: the MAP of
	 * the new one's region fails, so that the device holds nothing of the staging extent the
	 * store's fault moves the page to.
	 */
	done = sim_write(other, 0, "staged", 6);
	check(done == 6, "writing the other file: %zd", done);
	sim_device_fail(VIRTIO_EXTENT_T_MAP, 0, 1);
	check(!sim_fsync(other), "the first seal");
	pause_wait_failed();
	pause_at(PAUSE_MOVED | PAUSE_SEALING);
	store = sim_spawn("storer", storer, NULL);
	pause_wait(PAUSE_MOVED);
	/* A second seal, begun, shuts the gate before the fault. */
	seal = sim_spawn("sealer", sealer, NULL);
	pause_wait(PAUSE_SEALING);
	store->killed = true;
	pause_at(PAUSE_SEALING);
	wait_for(store);
	pause_at(0);
	wait_for(seal);

	/* A PTE of the PMD's folio, through the entry the PMD left. */
	err = sim_user_load(addr + PAGE_SIZE, got, 1);
	check(!err && got[0] == data[PAGE_SIZE], "a load of page 1: %d, %#x", err, got[0]);

	memset(written, 'w', sizeof(written));
	done = sim_write(file, 0, written, sizeof(written));
	check(done == sizeof(written), "the write of page 0: %zd", done);
	err = sim_user_load(addr, got, sizeof(got));
	check(!err && !memcmp(got, written, sizeof(got)), "a load of page 0 after the write: %d",
	      err);
	err = sim_user_load(addr + PAGE_SIZE, got, sizeof(got));
	check(!err && !memcmp(got, data + PAGE_SIZE, sizeof(got)), "a load of page 1: %d", err);
	done = sim_read(file, 0, got, sizeof(got));
	check(done == sizeof(got) && !memcmp(got, written, sizeof(got)), "a read of page 0: %zd",
	      done);
}

int main(void)
{
	sim_init(1);
	sim_device_init(64UL << 20);
	sim_spawn("scenario", scenario, NULL);
	sim_run();
	sim_log("pmd_entry_invalidated_mapped: passed\n");
	return 0;
}
