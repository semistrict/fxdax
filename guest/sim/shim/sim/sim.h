/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/* The simulator's own interface, which tests drive: tasks, the scheduler, and a mounted file. */
#ifndef _SIM_SIM_H
#define _SIM_SIM_H

#include <sim/module.h>
#include <sim/fs.h>
#include <sim/time.h>

#include "../../../linux/fs/fxfs/vfs.h"

/* Starts the simulator, whose scheduler `seed` drives: the same seed, the same run. */
void sim_init(u64 seed);
/* A new task, which runs `fn(arg)` once sim_run runs. */
struct task_struct *sim_spawn(const char *name, void (*fn)(void *arg), void *arg);
/* Runs every task to its end, and fails the run on a deadlock or a livelock. */
void sim_run(void);
/* A scheduling point: the seed may run another task here. */
void sim_point(const char *what);
/* The seeded generator tests take their own choices from: a part of the run. */
u64 sim_random(void);
/* A hash of every choice the scheduler made: runs that hash alike ran alike. */
u64 sim_trace_hash(void);
/* How many scheduling points the run reached, and how many switched task. */
u64 sim_points(void);
u64 sim_switches(void);
/* Has each scheduling point logged, with the task it ran. */
void sim_trace_on(void);

/*
 * Has `count` allocations by fs/fxfs's own sources fail, after `skip` more of them; or one in `odds`
 * of them, from the seed, until told 0: how many have.
 */
void sim_alloc_fail(u64 skip, u64 count);
void sim_alloc_fail_odds(unsigned int odds);
u64 sim_allocs_failed(void);

/* The fake device under the window: seals what is open, frees extents. */
void sim_device_init(u64 window_len);
void sim_device_new_extent(u64 sequence, u64 size);
void sim_device_seal(void);
void sim_device_free_extent(u64 sequence);
u64 sim_device_store_offset(u64 sequence);
u64 sim_device_maps(void);
/*
 * Has `count` requests of `type` (VIRTIO_EXTENT_T_MAP or _UNMAP) fail with EIO, doing nothing, after
 * `skip` more of them; or one MAP or UNMAP in `odds`, from the seed, until told 0: how many have.
 */
void sim_device_fail(u32 type, u64 skip, u64 count);
void sim_device_fail_odds(unsigned int odds);
u64 sim_device_failed(void);
/* Logs every MAP and UNMAP of a window page. */
void sim_device_watch(u64 page);
/* Logs what a window page, or a task's address, maps now. */
void sim_device_describe(u64 page);
void sim_mm_describe(unsigned long addr);
bool sim_mm_is_huge(unsigned long addr);
/* The address of a writable PTE of the window's `pages` pages from `page`, or 0. */
unsigned long sim_mm_writable_mapping(u64 page, u64 pages);
/* Logs what happens to the PTE of a task's address. */
void sim_mm_watch(unsigned long addr);

/* A mount, as fs/fxfs/super.c's, of the sim's Fxfs (fxfs/fxfs.c). */
struct sim_mount {
	struct super_block sb;
	struct fxfs_vfs_info *info;
};
struct sim_mount *sim_mount_new(void);
void sim_mount_free(struct sim_mount *mount);
struct file *sim_file_new(struct sim_mount *mount);
void sim_file_evict(struct file *file);
/* Evicts `file`'s inode, which nothing maps, and reads a new one in for it: that inode, opened. */
struct file *sim_file_read_in(struct sim_mount *mount, struct file *file);
/* What VFS calls do, each as a task's system call, into a kernel buffer. */
ssize_t sim_write(struct file *file, loff_t pos, const void *buf, size_t len);
ssize_t sim_read(struct file *file, loff_t pos, void *buf, size_t len);
/* As sim_write and sim_read, to or from a task's simulated memory. */
ssize_t sim_write_user(struct file *file, loff_t pos, unsigned long addr, size_t len);
ssize_t sim_read_user(struct file *file, loff_t pos, unsigned long addr, size_t len);
int sim_fsync(struct file *file);
int sim_truncate(struct file *file, loff_t size);
int sim_punch(struct file *file, loff_t start, loff_t end);
/* Has `len` page-aligned bytes of `out` share those of `in`, another file. */
int sim_clone(struct file *in, loff_t pos_in, struct file *out, loff_t pos_out, loff_t len);
/* Maps `len` bytes of the file at `offset`, shared, into the simulated mm: the address. */
unsigned long sim_mmap(struct file *file, loff_t offset, size_t len);
unsigned long sim_mm_map(struct mm_struct *mm, struct file *file, pgoff_t pgoff, size_t len);
void sim_munmap(unsigned long addr);
void sim_mm_init(void *window, u64 len);
unsigned long sim_window_pfn(pgoff_t pgoff);
/* How many faults tasks have taken. */
extern unsigned long sim_mm_faults;
/* Whether a WARN logs and goes on, rather than fails the run. */
extern bool sim_warns_continue;
/* A user access, a page at a time: 0, or -EFAULT for a SIGBUS or a SIGSEGV. */
int sim_user_store(unsigned long addr, const void *buf, size_t len);
int sim_user_load(unsigned long addr, void *buf, size_t len);
/* How many bytes it copied, without faulting if `pagefault_disabled`, as copy_{to,from}_user. */
size_t sim_copy_user(unsigned long addr, void *buf, size_t len, bool write);

/* The sim's Fxfs: reads the file as it would, from its extents, for the tests to check. */
void fxfs_sim_read(struct fxfs *fs, u64 ino, u64 offset, void *buf, size_t len);
u64 fxfs_sim_size(struct fxfs *fs, u64 ino);
u64 fxfs_sim_new_file(struct fxfs *fs);
struct fxfs *fxfs_sim_new(void);
void fxfs_sim_free(struct fxfs *fs);
/* How many blocks moved, extents were freed, extents a write opened again, transactions failed. */
void fxfs_sim_stats(u64 *moves, u64 *frees, u64 *failed);
/* Has one transaction in `odds` fail with -ENOSPC, or none if 0. */
void fxfs_sim_fail(unsigned int odds);
/* How many seals have ended. */
u64 fxfs_sim_seals(struct fxfs *fs);
/* Logs what happens to the block of `page` of file `ino`. */
void fxfs_sim_watch(u64 ino, u64 page);

#endif
