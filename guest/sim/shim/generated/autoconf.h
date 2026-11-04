/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/* The kernel's configuration, as the sources under test see it. */
#ifndef _SIM_AUTOCONF_H
#define _SIM_AUTOCONF_H

#define CONFIG_64BIT 1
#define CONFIG_ARM64 1
#define CONFIG_MMU 1
#define CONFIG_SMP 1
#define CONFIG_NR_CPUS 2
#define CONFIG_FS_DAX 1
#define CONFIG_FS_DAX_PMD 1
#define CONFIG_DAX 1
#define CONFIG_XARRAY_MULTI 1
#define CONFIG_TRANSPARENT_HUGEPAGE 1
/* Lockdep's assertions in the sources, which the tasks' held locks check: see sim/lockdep.h. */
#define CONFIG_LOCKDEP 1
#define CONFIG_ILLEGAL_POINTER_VALUE 0xdead000000000000
#define CONFIG_BASE_SMALL 0

#endif
