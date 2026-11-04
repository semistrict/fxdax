// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * The process under the fake kernel: the one file built against libc rather than the shim.
 * Coroutines are ucontexts, told to ASan as it asks. The window is one mapping of address space,
 * read-only but for private pages, and a store through it that the host would refuse is a real
 * SIGSEGV. One it would copy on write (doc-13) faults too, as the host's userfaultfd write-protect
 * does, and the device's model says why it refuses it: the guest makes every page private
 * (PRIVATE) before a store can reach it, so a store the host would copy at is a bug of the guest's
 * (TASK-164.9).
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
#include <sys/personality.h>
#include <sys/stat.h>
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
/*
 * Takes a store the window faulted on: NULL if it made the page private, or why the host
 * refused it.
 */
/* The device model's, whose u64 is unsigned long long: a pointer of another type is no call UBSan allows. */
static const char *(*window_store)(unsigned long long offset);

static void die(const char *what)
{
	fprintf(stderr, "sim host: %s: %s\n", what, strerror(errno));
	abort();
}

void host_fixed_addresses(void)
{
	static char arguments[4096];
	char *argv[32];
	int persona = personality(0xffffffff), argc = 0;
	ssize_t len, at = 0;
	int fd;

	if (persona & ADDR_NO_RANDOMIZE)
		return;
	if (personality(persona | ADDR_NO_RANDOMIZE) < 0)
		die("personality");
	fd = open("/proc/self/cmdline", O_RDONLY);
	if (fd < 0)
		die("open");
	len = read(fd, arguments, sizeof(arguments) - 1);
	close(fd);
	while (at < len && argc < 31) {
		argv[argc++] = &arguments[at];
		at += strlen(&arguments[at]) + 1;
	}
	argv[argc] = NULL;
	execv("/proc/self/exe", argv);
	die("execv");
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

/*
 * A store the window faults on may be one the host copies on write: the device's model makes the
 * page private, and the store, returned to, goes on. It runs in the middle of the store, with no
 * scheduling point, as the host's copy runs with the vCPU stopped.
 */
static void window_segv(int sig, siginfo_t *info, void *context)
{
	uintptr_t at = (uintptr_t)info->si_addr, base = (uintptr_t)window;
	const char *why = "nothing mapped private there";

	if (window && at >= base && at < base + window_len) {
		if (sig == SIGSEGV && window_store)
			why = window_store(at - base);
		if (!why)
			return;
		fprintf(stderr, "sim: the window refused an access at %#lx (%s): a host would have stopped the VM\n",
			at - base, why);
	} else
		fprintf(stderr, "sim: SIGSEGV at %#lx\n", at);
	__sanitizer_print_stack_trace();
	_exit(3);
}

void *host_window_init(uint64_t len, const char *(*store)(unsigned long long offset))
{
	struct sigaction action = { .sa_sigaction = window_segv, .sa_flags = SA_SIGINFO };

	/* 2 MiB aligned, as its pfns are, for PMD faults. */
	char *base = mmap(NULL, len + (2UL << 20), PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);

	if (base == MAP_FAILED)
		die("mmap");
	window = (char *)(((uintptr_t)base + (2UL << 20) - 1) & ~((2UL << 20) - 1));
	window_len = len;
	window_store = store;
	if (sigaction(SIGSEGV, &action, NULL) || sigaction(SIGBUS, &action, NULL))
		die("sigaction");
	return window;
}

void host_window_map(uint64_t offset, uint64_t store_offset, uint64_t len)
{
	if (mmap(window + offset, len, PROT_READ, MAP_SHARED | MAP_FIXED, store_fd, store_offset) ==
	    MAP_FAILED)
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

/*
 * The host's copy on write: memory of the VM's own in place of what the window maps, writable,
 * holding what it held, or zeros. The copy is made aside and moved into place, so that the window
 * never shows the page empty.
 */
void host_window_private(uint64_t offset, uint64_t len, bool zeros)
{
	char *copy;

	if (zeros) {
		if (mmap(window + offset, len, PROT_READ | PROT_WRITE,
			 MAP_PRIVATE | MAP_FIXED | MAP_ANONYMOUS, -1, 0) == MAP_FAILED)
			die("mmap of a private page of zeros");
		return;
	}
	copy = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	if (copy == MAP_FAILED)
		die("mmap of a private page");
	memcpy(copy, window + offset, len);
	if (mremap(copy, len, len, MREMAP_MAYMOVE | MREMAP_FIXED, window + offset) == MAP_FAILED)
		die("mremap of a private page");
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
