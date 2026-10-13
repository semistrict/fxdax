/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/*
 * What the kernel gives the filesystem's Rust (guest/rust/linux-rt and guest/rust/std): memory,
 * logging, panics, a way for a thread to wait for a future, work items, timers, the clocks and
 * randomness. Rust declares the same functions in linux-rt's ffi module and std's sys module.
 */
#ifndef _FS_FXFS_RT_H
#define _FS_FXFS_RT_H

#include <linux/types.h>

/* Memory for Rust's allocator, which may sleep: NULL if there is none, or `align` is too large. */
void *fxfs_rt_alloc(size_t size, size_t align);
void fxfs_rt_free(void *ptr);

/* Logs `len` bytes of `msg` at a printk level (0 to 7, KERN_EMERG to KERN_DEBUG). */
void fxfs_rt_log(int level, const char *msg, size_t len);
/* Panics the kernel, for a Rust panic. */
void __noreturn fxfs_rt_panic(const char *msg, size_t len);

/*
 * A parker lets a thread sleep until it is woken: each fxfs_rt_unpark lets one fxfs_rt_park
 * return. Unparking does not sleep, so it may be called from any context.
 */
struct fxfs_rt_parker;
struct fxfs_rt_parker *fxfs_rt_parker_new(void);
void fxfs_rt_parker_free(struct fxfs_rt_parker *parker);
void fxfs_rt_park(struct fxfs_rt_parker *parker);
void fxfs_rt_unpark(struct fxfs_rt_parker *parker);

/*
 * Calls `run(arg)` on a kernel worker, once. Does not sleep, so it may be called from any context;
 * fails with -ENOMEM if it cannot.
 */
int fxfs_rt_schedule(void (*run)(void *arg), void *arg);

/*
 * A one-shot timer, which calls `fire(arg)` from a softirq once started and its time has passed.
 * Freeing a timer cancels it, waiting for `fire` if it is running.
 */
struct fxfs_rt_timer;
struct fxfs_rt_timer *fxfs_rt_timer_new(void (*fire)(void *arg), void *arg);
void fxfs_rt_timer_start(struct fxfs_rt_timer *timer, u64 ns);
void fxfs_rt_timer_free(struct fxfs_rt_timer *timer);

/* The monotonic clock, in nanoseconds. */
u64 fxfs_rt_monotonic_ns(void);
/* The real-time clock, in nanoseconds since the Unix epoch. */
u64 fxfs_rt_realtime_ns(void);

/* Fills `buf` with `len` random bytes, from the kernel's generator. */
void fxfs_rt_random_bytes(void *buf, size_t len);

#endif
