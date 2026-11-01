// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * The process under the fake kernel: the one file built against libc rather than the shim.
 * Coroutines are ucontexts, told to ASan as it asks. The window is one mapping of address space,
 * and a store through it that the host would refuse is a real SIGSEGV.
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <sanitizer/asan_interface.h>
#include <sanitizer/common_interface_defs.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <ucontext.h>
#include <unistd.h>

#define FIBER_STACK (1 << 21)

struct sim_fiber {
	ucontext_t context;
	void *stack;
	size_t stack_size;
	void *fake_stack;
	void (*fn)(void *arg);
	void *arg;
};

static struct sim_fiber main_fiber;
static int store_fd = -1;
static char *window;
static uint64_t window_len;

static void die(const char *what)
{
	fprintf(stderr, "sim host: %s: %s\n", what, strerror(errno));
	abort();
}

void *host_malloc(size_t size)
{
	void *p = malloc(size ? size : 1);

	if (!p)
		die("malloc");
	return p;
}

void *host_realloc(void *p, size_t size)
{
	p = realloc(p, size ? size : 1);
	if (!p)
		die("realloc");
	return p;
}

void host_free(void *p)
{
	free(p);
}

static void fiber_entered(void)
{
	__sanitizer_finish_switch_fiber(NULL, NULL, NULL);
}

static void fiber_main(unsigned int low, unsigned int high)
{
	struct sim_fiber *fiber = (void *)(((uintptr_t)high << 32) | low);

	fiber_entered();
	fiber->fn(fiber->arg);
	abort();
}

struct sim_fiber *host_fiber_new(void (*fn)(void *arg), void *arg)
{
	struct sim_fiber *fiber = calloc(1, sizeof(*fiber));

	if (!fiber)
		die("calloc");
	if (posix_memalign(&fiber->stack, 4096, FIBER_STACK))
		die("posix_memalign");
	fiber->stack_size = FIBER_STACK;
	fiber->fn = fn;
	fiber->arg = arg;
	if (getcontext(&fiber->context))
		die("getcontext");
	fiber->context.uc_stack.ss_sp = fiber->stack;
	fiber->context.uc_stack.ss_size = FIBER_STACK;
	fiber->context.uc_link = NULL;
	makecontext(&fiber->context, (void (*)(void))fiber_main, 2, (unsigned int)(uintptr_t)fiber,
		    (unsigned int)((uintptr_t)fiber >> 32));
	return fiber;
}

struct sim_fiber *host_fiber_self(void)
{
	return &main_fiber;
}

void host_fiber_switch(struct sim_fiber *from, struct sim_fiber *to)
{
	void *bottom = to->stack;
	size_t size = to->stack_size;

	/* The main context's stack is the process's, which ASan knows already. */
	if (to == &main_fiber) {
		bottom = NULL;
		size = 0;
	}
	__sanitizer_start_switch_fiber(&from->fake_stack, bottom, size);
	if (swapcontext(&from->context, &to->context))
		die("swapcontext");
	__sanitizer_finish_switch_fiber(from->fake_stack, NULL, NULL);
}

void host_fiber_free(struct sim_fiber *fiber)
{
	free(fiber->stack);
	free(fiber);
}

void host_store_init(uint64_t len)
{
	store_fd = memfd_create("sim-store", MFD_CLOEXEC);
	if (store_fd < 0)
		die("memfd_create");
	if (ftruncate(store_fd, len))
		die("ftruncate");
}

void host_store_read(uint64_t offset, void *buf, size_t len)
{
	if (pread(store_fd, buf, len, offset) != (ssize_t)len)
		die("pread");
}

void host_store_write(uint64_t offset, const void *buf, size_t len)
{
	if (pwrite(store_fd, buf, len, offset) != (ssize_t)len)
		die("pwrite");
}

void host_store_zero(uint64_t offset, size_t len)
{
	if (fallocate(store_fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE, offset, len))
		die("fallocate");
}

static void window_segv(int sig, siginfo_t *info, void *context)
{
	uintptr_t at = (uintptr_t)info->si_addr, base = (uintptr_t)window;

	if (window && at >= base && at < base + window_len)
		fprintf(stderr, "sim: the window refused an access at %#lx: a host would have stopped the VM\n",
			at - base);
	else
		fprintf(stderr, "sim: SIGSEGV at %#lx\n", at);
	__sanitizer_print_stack_trace();
	_exit(3);
}

void *host_window_init(uint64_t len)
{
	struct sigaction action = { .sa_sigaction = window_segv, .sa_flags = SA_SIGINFO };

	/* 2 MiB aligned, as its pfns are, for PMD faults. */
	char *base = mmap(NULL, len + (2UL << 20), PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);

	if (base == MAP_FAILED)
		die("mmap");
	window = (char *)(((uintptr_t)base + (2UL << 20) - 1) & ~((2UL << 20) - 1));
	window_len = len;
	if (sigaction(SIGSEGV, &action, NULL) || sigaction(SIGBUS, &action, NULL))
		die("sigaction");
	return window;
}

void host_window_map(uint64_t offset, uint64_t store_offset, uint64_t len, bool writable)
{
	if (mmap(window + offset, len, PROT_READ | (writable ? PROT_WRITE : 0),
		 MAP_SHARED | MAP_FIXED, store_fd, store_offset) == MAP_FAILED)
		die("mmap of the store");
}

void host_window_map_zeros(uint64_t offset, uint64_t len)
{
	if (mmap(window + offset, len, PROT_READ, MAP_PRIVATE | MAP_FIXED | MAP_ANONYMOUS, -1, 0) ==
	    MAP_FAILED)
		die("mmap of zeros");
}

void host_window_unmap(uint64_t offset, uint64_t len)
{
	if (mmap(window + offset, len, PROT_NONE, MAP_PRIVATE | MAP_FIXED | MAP_ANONYMOUS, -1, 0) ==
	    MAP_FAILED)
		die("mmap of nothing");
}

void host_window_read_only(uint64_t offset, uint64_t len)
{
	if (mprotect(window + offset, len, PROT_READ))
		die("mprotect");
}

void host_stack_trace(void)
{
	__sanitizer_print_stack_trace();
}

_Noreturn void host_exit(int status)
{
	fflush(stdout);
	fflush(stderr);
	_exit(status);
}

void host_write_log(const char *line, size_t len)
{
	fwrite(line, 1, len, stderr);
}
