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
 * or a -errno (-ENOENT if there is no such device). Fxfs sizes what it keeps in memory by
 * the `memory` bytes the kernel has.
 */
int fxfs_mount(const char *source, size_t len, u64 memory, struct fxfs **fs,
	       struct fxfs_attr *root);
/* Unmounts the filesystem, writing back what is dirty. */
void fxfs_unmount(struct fxfs *fs);
/* Makes what the filesystem holds durable: sync, syncfs. */
int fxfs_sync(struct fxfs *fs);
/*
 * Makes what the filesystem holds survive the guest's crash, before what comes after: fsync, which
 * need not wait for the host's disk (doc-2's requirement 2).
 */
int fxfs_fsync(struct fxfs *fs);
int fxfs_statfs(struct fxfs *fs, struct fxfs_statfs *statfs);

/* Frees the blocks of file `ino` within the `len` bytes at `offset`, block-aligned: a hole. */
int fxfs_punch(struct fxfs *fs, u64 ino, u64 offset, u64 len);
/*
 * The attributes of inode `ino`, for a file handle: -ENOENT if no name links it and nothing has it
 * open, or it is gone.
 */
int fxfs_getattr(struct fxfs *fs, u64 ino, struct fxfs_attr *attr);
/* Sets the attributes `set` says. */
int fxfs_setattr(struct fxfs *fs, u64 ino, const struct fxfs_setattr *set);
/*
 * Writes back what the kernel keeps of inode `ino`: the attributes `set` says, and the changes to
 * its extended attributes, which fxfs_setxattr and fxfs_removexattr record.
 */
int fxfs_write_inode(struct fxfs *fs, u64 ino, const struct fxfs_setattr *set);

/* The attributes of `name` in directory `dir`: -ENOENT if there is none. */
int fxfs_lookup(struct fxfs *fs, u64 dir, const char *name, size_t len, struct fxfs_attr *attr);

/*
 * Calls `emit` for the entries of directory `dir`, in order, until `emit` returns false: those
 * after the entry named with the `after_len` bytes of `after`, which may have been removed since,
 * or without `after`, from the `index`th on.
 */
typedef bool (*fxfs_emit_t)(void *ctx, const char *name, size_t len, u64 ino, u32 mode);
int fxfs_readdir(struct fxfs *fs, u64 dir, u64 index, const char *after, size_t after_len,
		 fxfs_emit_t emit, void *ctx);

/*
 * Each change to a directory's names fills in the attributes of the inodes it changed, as it
 * left them: the directories', and those of the inodes it linked.
 */

/*
 * Creates `name` in directory `dir` as `mode` says: a directory, a regular file, or a special
 * file (FIFO, socket or device `rdev`), owned by `uid` and `gid`.
 */
int fxfs_create(struct fxfs *fs, u64 dir, const char *name, size_t len, u32 mode, u64 rdev,
		u32 uid, u32 gid, struct fxfs_attr *attr, struct fxfs_attr *dir_attr);
/* The longest target a symlink has: PATH_MAX - 1, as Fxfs's MAX_LONG_SYMLINK. */
#define FXFS_SYMLINK_MAX 4095

/*
 * Creates the symlink `name` in directory `dir`, to the `target_len` bytes of `target`, owned by
 * `uid` and `gid`.
 */
int fxfs_symlink(struct fxfs *fs, u64 dir, const char *name, size_t len, const char *target,
		 size_t target_len, u32 uid, u32 gid, struct fxfs_attr *attr,
		 struct fxfs_attr *dir_attr);
/* Copies up to `len` bytes of symlink `ino`'s target to `buf`, returning its length. */
ssize_t fxfs_readlink(struct fxfs *fs, u64 ino, char *buf, size_t len);
/*
 * Links file or symlink `ino` into directory `dir` as `name`: if `unnamed`, an unnamed file
 * (fxfs_tmpfile), which leaves the graveyard.
 */
int fxfs_link(struct fxfs *fs, u64 ino, u64 dir, const char *name, size_t len, bool unnamed,
	      struct fxfs_attr *attr, struct fxfs_attr *dir_attr);
/*
 * Creates an unnamed regular file, as `mode` says, owned by `uid` and `gid`, for directory `dir`
 * (O_TMPFILE): in the graveyard, which frees it once evicted unless a link names it first.
 */
int fxfs_tmpfile(struct fxfs *fs, u64 dir, u32 mode, u32 uid, u32 gid, struct fxfs_attr *attr);
/*
 * The kernel's locks keep which inode a name links; removing or replacing one, it says which:
 * Fxfs's lookups needn't find out first.
 */

/*
 * Removes `name`, which links inode `ino`, no directory, from directory `dir` later, in a batch:
 * before anything reads what it changes, and before syncs. 0, or a -errno.
 */
int fxfs_unlink_later(struct fxfs *fs, u64 dir, const char *name, size_t len, u64 ino);
/* Removes `name`, which links inode `ino`, from directory `dir`: FXFS_REPLACED_*, or a -errno. */
int fxfs_unlink(struct fxfs *fs, u64 dir, const char *name, size_t len, u64 ino,
		struct fxfs_attr *dir_attr);
/*
 * Moves `name`, which links inode `ino`, in `dir` to `new_name` in `new_dir`, replacing inode
 * `new_ino` there, 0 if none: FXFS_REPLACED_*, or a -errno. `attr` is the moved inode's.
 */
int fxfs_rename(struct fxfs *fs, u64 dir, const char *name, size_t len, u64 ino, u64 new_dir,
		const char *new_name, size_t new_len, u64 new_ino, struct fxfs_attr *attr,
		struct fxfs_attr *dir_attr, struct fxfs_attr *new_dir_attr);
/*
 * Exchanges `name` in `dir` and `new_name` in `new_dir`, which both exist (RENAME_EXCHANGE):
 * 0, or a -errno. `attr` is the inode `name` linked, and `new_attr` the one `new_name` did.
 */
int fxfs_exchange(struct fxfs *fs, u64 dir, const char *name, size_t len, u64 new_dir,
		  const char *new_name, size_t new_len, struct fxfs_attr *attr,
		  struct fxfs_attr *new_attr, struct fxfs_attr *dir_attr,
		  struct fxfs_attr *new_dir_attr);
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
	/*
	 * How far the extent reaches, as allocated, or may yet if it is the staging extent, which
	 * small files share: 0 if unknown.
	 */
	u64 extent_size;
};

/*
 * The pieces of file `ino`'s data within `len` bytes at `offset`, in order, at most `max`, and one
 * per block at most; holes have none. Returns how many, or a -errno.
 */
ssize_t fxfs_dax_extents(struct fxfs *fs, u64 ino, u64 offset, u64 len,
			 struct fxfs_dax_extent *out, size_t max);
/*
 * fxdax: replaces the staging extent, if a barrier sealed it since blocks were taken of it, with a
 * new one, open, which `out` describes (its extent and extent_size), for the window to map before
 * the blocks of the next writes move there: true if it did.
 */
bool fxfs_dax_restage(struct fxfs *fs, struct fxfs_dax_extent *out);
/*
 * Makes room to write `len` bytes at `offset` of file `ino` through the window, without writing
 * them, and makes the file `size` bytes long if it is shorter, in one transaction. Returns the
 * pieces holding the range's blocks, as fxfs_dax_extents, which are open: blocks of open extents
 * stay, and the others move to new ones, holding what the range's first and last blocks hold
 * around it: or, if the caller may store only some of the range (`keep`), as a fault does, all
 * they hold. The caller stores the data through writable mappings before the extents are sealed.
 */
ssize_t fxfs_dax_map_write(struct fxfs *fs, u64 ino, u64 offset, u64 len, u64 size, bool keep,
			   struct fxfs_dax_extent *out, size_t max);

/*
 * Extended attributes of inode `ino`, by their full names (`user.x`): getxattr(2)'s and
 * listxattr(2)'s semantics, ssize_t results as theirs (the length, or with `size` 0 the length
 * needed, or -ERANGE), and setxattr(2)'s flags. Changes are written back with the inode
 * (fxfs_write_inode).
 */
ssize_t fxfs_getxattr(struct fxfs *fs, u64 ino, const char *name, size_t len, void *buf,
		      size_t size);
int fxfs_setxattr(struct fxfs *fs, u64 ino, const char *name, size_t len, const void *value,
		  size_t value_len, int flags);
ssize_t fxfs_listxattr(struct fxfs *fs, u64 ino, char *buf, size_t size);
int fxfs_removexattr(struct fxfs *fs, u64 ino, const char *name, size_t len);

#endif
