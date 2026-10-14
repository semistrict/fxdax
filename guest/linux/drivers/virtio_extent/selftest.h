/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/*
 * The driver's self-test: writing to a device's `selftest` attribute in sysfs runs every request
 * of the specification on it, and reading the attribute gives the result, "ok" or what failed.
 */
#ifndef _VIRTIO_EXTENT_SELFTEST_H
#define _VIRTIO_EXTENT_SELFTEST_H

struct virtio_device;
struct virtio_extent;

int vext_selftest_add(struct virtio_extent *vext, struct virtio_device *vdev);
void vext_selftest_remove(struct virtio_device *vdev);

#endif
