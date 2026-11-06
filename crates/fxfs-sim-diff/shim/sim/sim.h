/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * What guest/sim/fxfs/fxfs.c, the sim's model of Fxfs, asks of guest/sim's kernel, over libc, for
 * a process of one thread that drives it beside fxfs-posix (crates/fxfs-sim-diff): no scheduler,
 * and so no lock ever waits. A lock taken while held is an operation the harness ran where the
 * model's locks would have had it wait (sim_point), which fails the run.
 */
#ifndef _SHIM_SIM_SIM_H
#define _SHIM_SIM_SIM_H

#include <linux/types.h>
#include <stdlib.h>
#include <string.h>

#define PAGE_SHIFT 12
#define PAGE_SIZE (1UL << PAGE_SHIFT)
#define SECTOR_SHIFT 9
#define S_IFREG 0100000

#define ENOMEM 12
#define EIO 5
#define E2BIG 7
#define ENOSPC 28

typedef unsigned int gfp_t;
#define GFP_KERNEL 0u
#define GFP_NOFS 0u

#define offset_in_page(p) ((unsigned long)(p) & (PAGE_SIZE - 1))
#define DIV_ROUND_UP(n, d) (((n) + (d) - 1) / (d))
#define min(a, b) ((a) < (b) ? (a) : (b))
#define max(a, b) ((a) > (b) ? (a) : (b))
#define min_t(t, a, b) min((t)(a), (t)(b))
#define max_t(t, a, b) max((t)(a), (t)(b))
#define swap(a, b) do { __typeof__(a) __swap = (a); (a) = (b); (b) = __swap; } while (0)
#define container_of(ptr, type, member) ((type *)((char *)(ptr) - offsetof(type, member)))

__attribute__((noreturn, format(printf, 1, 2))) void sim_panic(const char *fmt, ...);
__attribute__((format(printf, 1, 2))) void sim_log(const char *fmt, ...);

/* Memory: the kernel's calls, which never fail here. */
void *kzalloc(size_t size, gfp_t gfp);
void *kmalloc(size_t size, gfp_t gfp);
void *kcalloc(size_t n, size_t size, gfp_t gfp);
void *krealloc(void *p, size_t size, gfp_t gfp);
void kfree(const void *p);

struct mutex {
	bool held;
};
struct rw_semaphore {
	unsigned int readers;
	bool writer;
};
void mutex_init(struct mutex *lock);
void mutex_lock(struct mutex *lock);
void mutex_unlock(struct mutex *lock);
void init_rwsem(struct rw_semaphore *sem);
void down_read(struct rw_semaphore *sem);
void up_read(struct rw_semaphore *sem);
void down_write(struct rw_semaphore *sem);
void up_write(struct rw_semaphore *sem);

/* The kernel's list.h, as much of it as the model uses. */
struct list_head {
	struct list_head *next, *prev;
};
#define INIT_LIST_HEAD(head) do { (head)->next = (head); (head)->prev = (head); } while (0)
static inline void list_add(struct list_head *entry, struct list_head *head)
{
	entry->next = head->next;
	entry->prev = head;
	head->next->prev = entry;
	head->next = entry;
}
static inline void list_del(struct list_head *entry)
{
	entry->prev->next = entry->next;
	entry->next->prev = entry->prev;
	entry->next = entry->prev = NULL;
}
#define list_entry(ptr, type, member) container_of(ptr, type, member)
#define list_for_each_entry(pos, head, member)                                         \
	for (pos = list_entry((head)->next, __typeof__(*pos), member); &pos->member != (head); \
	     pos = list_entry(pos->member.next, __typeof__(*pos), member))
#define list_for_each_entry_safe(pos, n, head, member)                                  \
	for (pos = list_entry((head)->next, __typeof__(*pos), member),                     \
	     n = list_entry(pos->member.next, __typeof__(*pos), member);                   \
	     &pos->member != (head);                                                       \
	     pos = n, n = list_entry(n->member.next, __typeof__(*n), member))

/* The task running: the harness's, which has a name alone. */
struct task_struct {
	char comm[16];
};
extern struct task_struct sim_task;
#define current (&sim_task)

/* No clock and no sleeps: nothing else runs meanwhile but what sim_point runs. */
static inline u64 ktime_get_ns(void)
{
	return 0;
}
static inline void msleep(unsigned int ms)
{
	(void)ms;
}

/*
 * A scheduling point of the sim's, where its scheduler may run another task: here, where the
 * harness may run an operation of its own, as the model's locks allow (crates/fxfs-sim-diff).
 */
void sim_point(const char *what);
u64 sim_random(void);
/* guest/sim's sweep (kernel/sweep.c): no call fails here, as the comparison with Fxfs has none. */
bool sim_sweep_fails(const char *what, const char *file, int line);

/* The extent store under the model, as guest/sim's kernel/device.c has it. */
void sim_device_new_extent(u64 sequence, u64 size);
void sim_device_seal(void);
void sim_device_free_extent(u64 sequence);
u64 sim_device_store_offset(u64 sequence);

/* The model's own interface, as guest/sim's sim.h declares it. */
struct fxfs;
void fxfs_sim_read(struct fxfs *fs, u64 ino, u64 offset, void *buf, size_t len);
u64 fxfs_sim_size(struct fxfs *fs, u64 ino);
u64 fxfs_sim_new_file(struct fxfs *fs);
struct fxfs *fxfs_sim_new(void);
void fxfs_sim_free(struct fxfs *fs);
void fxfs_sim_stats(u64 *written, u64 *frees, u64 *failed);
void fxfs_sim_fail(unsigned int odds);
void fxfs_sim_hold_commits(bool hold);
bool fxfs_sim_commit_held(void);
void fxfs_sim_hold_seals(bool hold);
bool fxfs_sim_seal_held(void);
void fxfs_sim_read_pending(struct fxfs *fs, u64 ino, u64 offset, void *buf, size_t len);
u64 fxfs_sim_reserved(struct fxfs *fs, u64 ino);
u64 fxfs_sim_seals(struct fxfs *fs);
bool fxfs_sim_sealing(struct fxfs *fs);
void fxfs_sim_watch(u64 ino, u64 page);

#endif
