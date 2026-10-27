/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
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
	/* With the inode's attribute flags above the kernel's bits (FXFS_MODE_*). */
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

/*
 * fxdax: the attribute flags (chattr) an inode's mode keeps above the kernel's 16 bits: Fxfs's
 * objects have no record of them of their own, and its mode is 32 bits.
 */
#define FXFS_MODE_IMMUTABLE (1u << 16)
#define FXFS_MODE_APPEND (1u << 17)
#define FXFS_MODE_NOATIME (1u << 18)
#define FXFS_MODE_NODUMP (1u << 19)

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
/* A directory, which no name links any more, until the kernel evicts it. */
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
 * the `memory` bytes the kernel has. fxdax: read-only if `read_only`, or if the device follows a
 * store another VM writes, when the filesystem follows that writer (fxfs_follow) on the
 * superblock `vfs`'s primitives (fxfs_vfs_follow_*, below).
 */
int fxfs_mount(const char *source, size_t len, u64 memory, bool read_only, void *vfs,
	       struct fxfs **fs, struct fxfs_attr *root);
/* fxdax: whether the filesystem is mounted read-only, and whether it follows its writer. */
bool fxfs_read_only(struct fxfs *fs);
bool fxfs_follows(struct fxfs *fs);
/*
 * fxdax: a follower's batch (backlog doc-8): the writer's commits since those applied, fetched;
 * holding the fill barrier, what the kernel caches of what they change dropped
 * (fxfs_vfs_drop_file), the commits applied, and the kernel's inodes settled
 * (fxfs_vfs_settle_file); then the commit it was as of let go of. 0, or a -errno (-ESTALE once
 * the follower is cut off: it remounts).
 */
int fxfs_follow(struct fxfs *fs);
/*
 * fxdax: holds the file `ino`, which the kernel opens, on a follower: one the writer deletes reads
 * as it was until the last open closes (fxfs_release). 0, or a -errno: -ENOENT if a batch deleted
 * it since the open looked it up, which holds nothing.
 */
int fxfs_hold(struct fxfs *fs, u64 ino);
void fxfs_release(struct fxfs *fs, u64 ino);

/*
 * fxdax: a follower's primitives, of the superblock `vfs` (super.c), which the filesystem's Rust
 * runs its protocol on (crates/follower): its fill barrier, held shared by a reader
 * (fxfs_vfs_fill_begin, vfs.h), which a tree read checks it holds, in a debug kernel.
 */
void fxfs_vfs_follow_fill_begin(void *vfs);
void fxfs_vfs_follow_fill_end(void *vfs);
void fxfs_vfs_follow_assert_filling(void *vfs);
/* The follower's state is as of a commit no later than `epoch`'s: its batch applied. */
#define FXFS_FOLLOW_APPLIED 0
/* The follower's state is as of a commit no earlier than `epoch`'s: its batch settled. */
#define FXFS_FOLLOW_SETTLED 1
void fxfs_vfs_follow_epoch(void *vfs, u32 which, u64 epoch);
/*
 * fxdax: a follower's batch takes its fill barrier exclusively, waiting for the readers holding it,
 * from before it drops what it changes until it has applied and settled it; and lets go of it.
 */
void fxfs_vfs_follow_barrier_take(void *vfs);
void fxfs_vfs_follow_barrier_release(void *vfs);

/* fxdax: bytes of a follower's file a batch changed, from `start` to `end`, exclusive. */
struct fxfs_range {
	u64 start;
	u64 end;
};
/*
 * fxdax: a follower's batch, holding its fill barrier (fxfs_vfs_follow_barrier_take), drops what
 * the kernel keeps of what it changes before it applies the change: the `count` `ranges` of the
 * file `ino`'s data; the dentries of the directory `ino`, which revalidate. Once applied, the file
 * `ino`'s inode takes its attributes, `attr`, or, `gone`, none, no name linking it.
 */
void fxfs_vfs_drop_file(void *vfs, u64 ino, const struct fxfs_range *ranges, size_t count);
void fxfs_vfs_drop_entries(void *vfs, u64 ino);
void fxfs_vfs_settle_file(void *vfs, u64 ino, const struct fxfs_attr *attr, bool gone);
/* Unmounts the filesystem, writing back what is dirty. */
void fxfs_unmount(struct fxfs *fs);
/* Makes what the filesystem holds durable: sync, syncfs. */
int fxfs_sync(struct fxfs *fs);
/*
 * fxdax: freezes the filesystem, which the VFS has synced, its writers stopped: makes it durable,
 * holds its background work, and closes the device to writes until it thaws, nothing open on it;
 * -EBUSY if writes kept coming. Thawing, it takes the device's branch, a fork's of its own in a VM
 * restored from a snapshot taken frozen, and writes again (TASK-39.1).
 */
int fxfs_freeze(struct fxfs *fs);
int fxfs_thaw(struct fxfs *fs);
/*
 * Makes what the filesystem holds survive the guest's crash, before what comes after: fsync, which
 * need not wait for the host's disk (doc-2's requirement 2).
 */
int fxfs_fsync(struct fxfs *fs);
/*
 * Takes the size the host resized the store to, as the device's configuration says, while mounted:
 * grows the filesystem, or, asked to shrink, shrinks it if it holds no more, and answers.
 */
int fxfs_resize(struct fxfs *fs);
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

/* fxdax: an extended attribute a new inode is made with (fxfs_create): its full name, its value. */
struct fxfs_xattr {
	const char *name;
	size_t name_len;
	const void *value;
	size_t value_len;
};

/*
 * Creates `name` in directory `dir` as `mode` says: a directory, a regular file, or a special
 * file (FIFO, socket or device `rdev`), owned by `uid` and `gid`. fxdax: with the `nr_xattrs`
 * extended attributes `xattrs`, the ACLs it inherits, in the same transaction (TASK-79.29).
 */
int fxfs_create(struct fxfs *fs, u64 dir, const char *name, size_t len, u32 mode, u64 rdev,
		u32 uid, u32 gid, const struct fxfs_xattr *xattrs, size_t nr_xattrs,
		struct fxfs_attr *attr, struct fxfs_attr *dir_attr);
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
 * fxdax: makes file `dst`'s `len` bytes at `dst_offset` share the extents holding file `src`'s at
 * `offset` (a reflink), copying nothing, and `dst` `size` bytes long if it is shorter: its
 * attributes in `attr`. The offsets and `len` are block aligned. -EOPNOTSUPP for extents written
 * in place, which can't be shared.
 */
int fxfs_clone_range(struct fxfs *fs, u64 src, u64 offset, u64 len, u64 dst, u64 dst_offset,
		     u64 size, struct fxfs_attr *attr);
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
 * fxdax: how many records Fxfs caches (its objects', names' and data's), and frees up to `count` of
 * them, those used least recently first, returning how many it freed: the superblock's shrinker's.
 */
u64 fxfs_cached_objects(struct fxfs *fs);
u64 fxfs_free_cached_objects(struct fxfs *fs, u64 count);
/*
 * fxdax: writes what the filesystem waited on, and its device's requests took, as lines of a name
 * and a number, into the `len` bytes at `buf`, as much as fits; returns how long it is (TASK-91.7).
 */
size_t fxfs_stats(struct fxfs *fs, char *buf, size_t len);

/*
 * Closes inode `ino`, which the kernel evicts, and frees it if `unlinked`: if no name links it, a
 * file's or a directory's, removed.
 */
void fxfs_evict(struct fxfs *fs, u64 ino, bool unlinked);

/* Reads up to `len` bytes of file `ino` at `offset` into `buf`, returning how many it read. */
ssize_t fxfs_read(struct fxfs *fs, u64 ino, u64 offset, void *buf, size_t len);
/* One of fxfs_read_into's buffers: room for `len` bytes at `buf`. */
struct fxfs_read_part {
	void *buf;
	size_t len;
};

/*
 * fxdax: reads file `ino` from `offset` into the `count` buffers `parts`, one after another, as
 * fxfs_read into one, but from the device's transfer buffer straight into them: readahead's folios
 * (TASK-92.9). Returns how many bytes it read.
 */
ssize_t fxfs_read_into(struct fxfs *fs, u64 ino, u64 offset, const struct fxfs_read_part *parts,
		       size_t count);
/*
 * Writes the `len` bytes of `buf` to file `ino` at `offset`, growing the file if it ends before
 * them. The file's times are the kernel's to set (fxfs_setattr).
 */
int fxfs_write(struct fxfs *fs, u64 ino, u64 offset, const void *buf, size_t len);

/* One of fxfs_write_many's writes: the `len` bytes at `data`, to `offset` in the file. */
struct fxfs_write_part {
	u64 offset;
	const void *data;
	size_t len;
};

/*
 * Writes each of the `count` parts of `parts` to file `ino`, as fxfs_write writes one, in as few
 * transactions as hold them, each with one write to the device: the page cache's writeback of a
 * file's folios, contiguous or not, in any order. No two share a block. fxdax: `dirty` bytes of
 * the file's dirty pages, which they write, have room reserved (fxfs_reserve_dirty), which pays
 * for them: a filesystem filled since doesn't fail them (TASK-80).
 */
int fxfs_write_many(struct fxfs *fs, u64 ino, const struct fxfs_write_part *parts, size_t count,
		    u64 dirty);
/*
 * fxdax: reserves room for `bytes` more of file `ino`'s data, whole pages, which the page cache
 * dirties: 0, or -ENOSPC if the filesystem hasn't room for them, and for writing them back
 * (TASK-80).
 */
int fxfs_reserve_dirty(struct fxfs *fs, u64 ino, u64 bytes);
/* fxdax: as fxfs_reserve_dirty, for pages dirtied whether the filesystem has room or not. */
void fxfs_reserve_dirty_what_fits(struct fxfs *fs, u64 ino, u64 bytes);
/* fxdax: releases the room of `bytes` of file `ino`'s dirty pages, dropped unwritten. */
void fxfs_release_dirty(struct fxfs *fs, u64 ino, u64 bytes);

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
 * they hold; fxdax: or `held`, if not NULL, `len` bytes, whole blocks, written in the transaction,
 * as a fault holds its page (TASK-79.11). The caller stores the data through writable mappings
 * before the extents are sealed.
 */
ssize_t fxfs_dax_map_write(struct fxfs *fs, u64 ino, u64 offset, u64 len, u64 size, bool keep,
			   const void *held, struct fxfs_dax_extent *out, size_t max);
/* Records an append already written through the window, without zeroing it (TASK-92.28). */
int fxfs_dax_update_size(struct fxfs *fs, u64 ino, u64 size);

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
