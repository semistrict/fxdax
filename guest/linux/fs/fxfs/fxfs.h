/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/*
 * The filesystem, in Rust (guest/rust/fxfs-linux), as the VFS glue in super.c calls it. Each call
 * may sleep. Errors are negative errnos.
 */
#ifndef _FS_FXFS_FXFS_H
#define _FS_FXFS_FXFS_H

#include <linux/types.h>

#define FXFS_ROOT_INO 1

/* A mounted filesystem. */
struct fxfs;

/* An inode's attributes. */
struct fxfs_attr {
	u64 ino;
	u64 size;
	u32 mode;
	u32 nlink;
};

/* Readies Rust's runtime, once, before any other call. */
void fxfs_init(void);

/* Mounts a filesystem, or returns NULL if there is no memory for one. */
struct fxfs *fxfs_mount(void);
void fxfs_unmount(struct fxfs *fs);

int fxfs_getattr(struct fxfs *fs, u64 ino, struct fxfs_attr *attr);
/* The attributes of `name` in directory `dir`: -ENOENT if there is none. */
int fxfs_lookup(struct fxfs *fs, u64 dir, const char *name, size_t len, struct fxfs_attr *attr);

/*
 * Calls `emit` for the entries of directory `dir` from the `index`th on, until `emit` returns
 * false.
 */
typedef bool (*fxfs_emit_t)(void *ctx, const char *name, size_t len, u64 ino, u32 mode);
int fxfs_readdir(struct fxfs *fs, u64 dir, u64 index, fxfs_emit_t emit, void *ctx);

/* Reads up to `len` bytes of file `ino` at `offset` into `buf`, returning how many it read. */
ssize_t fxfs_read(struct fxfs *fs, u64 ino, u64 offset, void *buf, size_t len);

#endif
