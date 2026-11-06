/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * What the fake kernel asks of the process it runs in (kernel/host.c, the only file that sees
 * libc's headers): memory, coroutines, and the window, which is real memory protection.
 */
#ifndef _SIM_HOST_H
#define _SIM_HOST_H

#include <sim/base.h>

/*
 * Runs the process again with its addresses fixed, unless they are: fs/dax.c hashes pointers into
 * its wait table, whose collisions change what a wake does, so the same seed needs the same
 * addresses to be the same run.
 */
void host_fixed_addresses(void);

void *host_malloc(size_t size);
void *host_realloc(void *p, size_t size);
void host_free(void *p);

struct sim_fiber;
/* A coroutine that runs `fn(arg)`, which must never return, once switched to. */
struct sim_fiber *host_fiber_new(void (*fn)(void *arg), void *arg);
/* The caller's own context, as a fiber to switch back to. */
struct sim_fiber *host_fiber_self(void);
void host_fiber_switch(struct sim_fiber *from, struct sim_fiber *to);
void host_fiber_free(struct sim_fiber *fiber);

/* The store's memory: a memfd of `len` bytes, all zeros. */
void host_store_init(u64 len);
void host_store_read(u64 offset, void *buf, size_t len);
void host_store_write(u64 offset, const void *buf, size_t len);
void host_store_zero(u64 offset, size_t len);

/*
 * The window: `len` bytes of address space, inaccessible until mapped, and mapped read-only. A
 * store it faults on calls `store(offset)`, in the middle of the store, which may make the page
 * private and return NULL, the store then going on; or returns why the host refuses it, which
 * stops the run.
 */
void *host_window_init(u64 len, const char *(*store)(u64 offset));
void host_window_map(u64 offset, u64 store_offset, u64 len);
void host_window_map_zeros(u64 offset, u64 len);
void host_window_unmap(u64 offset, u64 len);
/* Makes `len` bytes at `offset` the VM's own, writable: holding what they held, or zeros. */
void host_window_private(u64 offset, u64 len, bool zeros);

void host_stack_trace(void);
__noreturn void host_exit(int status);
/* Has `fn` run as the process exits, as host_end ends it, after which LeakSanitizer looks. */
void host_at_exit(void (*fn)(void));
__noreturn void host_end(int status);
void host_write_log(const char *line, size_t len);
/* The process's environment's `name`, or NULL. */
const char *host_getenv(const char *name);

#endif
