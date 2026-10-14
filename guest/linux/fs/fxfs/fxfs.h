/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/*
 * The filesystem, in Rust (guest/rust/fxfs-linux), as the VFS glue in super.c calls it. Each call
 * may sleep. Errors are negative errnos.
 */
#ifndef _FS_FXFS_FXFS_H
#define _FS_FXFS_FXFS_H

#include <linux/types.h>

/* A mounted filesystem. */
struct fxfs;

/* An inode's attributes. Its inode number is its object's ID in Fxfs. */
struct fxfs_attr {
	u64 ino;
	u64 size;
	/* 512-byte blocks the inode's data takes. */
	u64 blocks;
	/* Nanoseconds since the Unix epoch. */
	u64 atime_ns;
	u64 mtime_ns;
	u64 ctime_ns;
	u32 mode;
	u32 nlink;
	u32 uid;
	u32 gid;
};

/* Readies Rust's runtime, once, before any other call. */
void fxfs_init(void);

/*
 * Mounts Fxfs on the extent-store device tagged with the `len` bytes of `source`, and fills in the
 * filesystem and its root directory's attributes: 0, or a -errno (-ENOENT if there is no such
 * device).
 */
int fxfs_mount(const char *source, size_t len, struct fxfs **fs, struct fxfs_attr *root);
/* Unmounts the filesystem, writing back what is dirty. */
void fxfs_unmount(struct fxfs *fs);

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
