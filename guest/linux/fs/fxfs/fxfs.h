/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/*
 * The filesystem, in Rust (guest/rust/fxfs-linux), as the VFS glue in super.c calls it. Each call
 * may sleep. Errors are negative errnos. The VFS checks what it can first: that names exist or
 * not, that types match, and permissions.
 */
#ifndef _FS_FXFS_FXFS_H
#define _FS_FXFS_FXFS_H

#include <linux/types.h>

/* A mounted filesystem. */
struct fxfs;
struct fxfs_device;

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
	/* A device file's device. */
	u64 rdev;
};

/* Which of `struct fxfs_setattr`'s attributes to set. */
#define FXFS_SET_MODE (1 << 0)
#define FXFS_SET_UID (1 << 1)
#define FXFS_SET_GID (1 << 2)
#define FXFS_SET_SIZE (1 << 3)
#define FXFS_SET_ATIME (1 << 4)
#define FXFS_SET_MTIME (1 << 5)
/* Otherwise a change's ctime is the time of the change. */
#define FXFS_SET_CTIME (1 << 6)

/* Attributes to set: those `valid` says. */
struct fxfs_setattr {
	u32 valid;
	u32 mode;
	u32 uid;
	u32 gid;
	u64 size;
	u64 atime_ns;
	u64 mtime_ns;
	u64 ctime_ns;
};

/* What fxfs_unlink and fxfs_rename removed or replaced. */
#define FXFS_REPLACED_NONE 0
/* A file or symlink, which other names may still link. */
#define FXFS_REPLACED_OBJECT 1
/* A directory, which is gone. */
#define FXFS_REPLACED_DIRECTORY 2

/* The filesystem's usage. */
struct fxfs_statfs {
	u64 block_size;
	u64 blocks;
	u64 free_blocks;
	u64 files;
};

/* Readies Rust's runtime, once, before any other call. */
void fxfs_init(void);

/*
 * Mounts Fxfs on the extent-store device tagged with the `len` bytes of `source`, formatting it if
 * its store holds nothing yet, and fills in the filesystem and its root directory's attributes: 0,
 * or a -errno (-ENOENT if there is no such device).
 */
int fxfs_mount(const char *source, size_t len, struct fxfs **fs, struct fxfs_attr *root);
/* Unmounts the filesystem, writing back what is dirty. */
void fxfs_unmount(struct fxfs *fs);
/* Makes what the filesystem holds durable. */
int fxfs_sync(struct fxfs *fs);
int fxfs_statfs(struct fxfs *fs, struct fxfs_statfs *statfs);

int fxfs_getattr(struct fxfs *fs, u64 ino, struct fxfs_attr *attr);
/* Sets the attributes `set` says, and fills in `attr` with the inode's. */
int fxfs_setattr(struct fxfs *fs, u64 ino, const struct fxfs_setattr *set, struct fxfs_attr *attr);

/* The attributes of `name` in directory `dir`: -ENOENT if there is none. */
int fxfs_lookup(struct fxfs *fs, u64 dir, const char *name, size_t len, struct fxfs_attr *attr);

/*
 * Calls `emit` for the entries of directory `dir` from the `index`th on, until `emit` returns
 * false.
 */
typedef bool (*fxfs_emit_t)(void *ctx, const char *name, size_t len, u64 ino, u32 mode);
int fxfs_readdir(struct fxfs *fs, u64 dir, u64 index, fxfs_emit_t emit, void *ctx);

/*
 * Creates `name` in directory `dir` as `mode` says: a directory, a regular file, or a special
 * file (FIFO, socket or device `rdev`), owned by `uid` and `gid`.
 */
int fxfs_create(struct fxfs *fs, u64 dir, const char *name, size_t len, u32 mode, u64 rdev,
		u32 uid, u32 gid, struct fxfs_attr *attr);
/* The longest target a symlink has: PATH_MAX - 1, as Fxfs's MAX_LONG_SYMLINK. */
#define FXFS_SYMLINK_MAX 4095

/*
 * Creates the symlink `name` in directory `dir`, to the `target_len` bytes of `target`, owned by
 * `uid` and `gid`.
 */
int fxfs_symlink(struct fxfs *fs, u64 dir, const char *name, size_t len, const char *target,
		 size_t target_len, u32 uid, u32 gid, struct fxfs_attr *attr);
/* Copies up to `len` bytes of symlink `ino`'s target to `buf`, returning its length. */
ssize_t fxfs_readlink(struct fxfs *fs, u64 ino, char *buf, size_t len);
/* Links file or symlink `ino` into directory `dir` as `name`. */
int fxfs_link(struct fxfs *fs, u64 ino, u64 dir, const char *name, size_t len,
	      struct fxfs_attr *attr);
/* Removes `name` from directory `dir`: FXFS_REPLACED_*, or a -errno. */
int fxfs_unlink(struct fxfs *fs, u64 dir, const char *name, size_t len);
/* Moves `name` in `dir` to `new_name` in `new_dir`, replacing it: FXFS_REPLACED_*, or a -errno. */
int fxfs_rename(struct fxfs *fs, u64 dir, const char *name, size_t len, u64 new_dir,
		const char *new_name, size_t new_len);
/*
 * Exchanges `name` in `dir` and `new_name` in `new_dir`, which both exist (RENAME_EXCHANGE):
 * 0, or a -errno.
 */
int fxfs_exchange(struct fxfs *fs, u64 dir, const char *name, size_t len, u64 new_dir,
		  const char *new_name, size_t new_len);
/*
 * Closes inode `ino`, which the kernel evicts, and frees it if `unlinked`: if no name links it,
 * and it is no directory, which goes as it is removed.
 */
void fxfs_evict(struct fxfs *fs, u64 ino, bool unlinked);

/* Reads up to `len` bytes of file `ino` at `offset` into `buf`, returning how many it read. */
ssize_t fxfs_read(struct fxfs *fs, u64 ino, u64 offset, void *buf, size_t len);
/*
 * Writes the `len` bytes of `buf` to file `ino` at `offset`, growing the file if it ends before
 * them. The file's times are the kernel's to set (fxfs_setattr).
 */
int fxfs_write(struct fxfs *fs, u64 ino, u64 offset, const void *buf, size_t len);

/* The extent-store device the filesystem is on (device.h). */
struct fxfs_device *fxfs_dax_device(struct fxfs *fs);

/*
 * A piece of a file's data, which the DAX window maps (dax.c): `len` bytes at `offset` in the file,
 * at `extent_offset` in the extent {branch, sequence}, which is open if `open`: only a writable
 * mapping maps an open extent. Offsets and lengths are multiples of the block, the page.
 */
struct fxfs_dax_extent {
	u64 offset;
	u64 len;
	u64 branch;
	u64 sequence;
	u64 extent_offset;
	u32 open;
	u32 reserved;
};

/*
 * The pieces of file `ino`'s data within `len` bytes at `offset`, in order, at most `max`, and one
 * per block at most; holes have none. Returns how many, or a -errno.
 */
ssize_t fxfs_dax_extents(struct fxfs *fs, u64 ino, u64 offset, u64 len,
			 struct fxfs_dax_extent *out, size_t max);
/*
 * Makes room to write `len` bytes at `offset` of file `ino` through the window, without writing
 * them, and makes the file `size` bytes long if it is shorter, in one transaction. Returns the
 * pieces holding the range's blocks, as fxfs_dax_extents, which are open: blocks of open extents
 * stay, and the others move to new ones, holding what the range's first and last blocks hold
 * around it. The caller stores the data through writable mappings before the extents are sealed.
 */
ssize_t fxfs_dax_map_write(struct fxfs *fs, u64 ino, u64 offset, u64 len, u64 size,
			   struct fxfs_dax_extent *out, size_t max);

/*
 * Extended attributes of inode `ino`, by their full names (`user.x`): getxattr(2)'s and
 * listxattr(2)'s semantics, ssize_t results as theirs (the length, or with `size` 0 the length
 * needed, or -ERANGE), and setxattr(2)'s flags.
 */
ssize_t fxfs_getxattr(struct fxfs *fs, u64 ino, const char *name, size_t len, void *buf,
		      size_t size);
int fxfs_setxattr(struct fxfs *fs, u64 ino, const char *name, size_t len, const void *value,
		  size_t value_len, int flags);
ssize_t fxfs_listxattr(struct fxfs *fs, u64 ino, char *buf, size_t size);
int fxfs_removexattr(struct fxfs *fs, u64 ino, const char *name, size_t len);

#endif
