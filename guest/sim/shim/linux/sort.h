/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * The kernel's sort(), as lib/sort.c's: in place, not stable, with no scheduling point. An
 * insertion sort, not the kernel's heapsort, which orders what compares equal otherwise: dax.c
 * sorts nothing whose order among equals matters (fxfs_dax_relieve, TASK-164.10).
 */
#ifndef _SIM_SORT_H
#define _SIM_SORT_H

#include <sim/base.h>

typedef int (*cmp_func_t)(const void *a, const void *b);
typedef void (*swap_func_t)(void *a, void *b, int size);

static inline void sort(void *base, size_t num, size_t size, cmp_func_t cmp, swap_func_t swap)
{
	char *items = base, held[64];
	size_t i, j;

	BUG_ON(size > sizeof(held) || swap);
	for (i = 1; i < num; i++) {
		memcpy(held, items + i * size, size);
		for (j = i; j > 0 && cmp(items + (j - 1) * size, held) > 0; j--)
			memcpy(items + j * size, items + (j - 1) * size, size);
		memcpy(items + j * size, held, size);
	}
}

#endif
