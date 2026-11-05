/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/* The store's memory, at offsets the device gives each extent (sim_device_store_offset). */
#ifndef _SHIM_SIM_HOST_H
#define _SHIM_SIM_HOST_H

#include <sim/sim.h>

void host_store_read(u64 offset, void *buf, size_t len);
void host_store_write(u64 offset, const void *buf, size_t len);

#endif
