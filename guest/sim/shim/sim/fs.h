/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * The VFS, faked at the level dax.c meets it: inodes, files and their mappings, kept by the sim's
 * own mount (kernel/vfs.c), and iov_iters over a kernel buffer or a task's simulated memory.
 */
#ifndef _SIM_FS_H
#define _SIM_FS_H

#include <sim/mm.h>
#include <sim/sched.h>
#include <linux/hash.h>
#include <linux/xarray.h>
#include <linux/wait.h>
#include <linux/wait_bit.h>

#define S_IFMT 00170000
#define S_IFREG 0100000
#define S_IFCHR 0020000
#define S_ISREG(m) (((m) & S_IFMT) == S_IFREG)
#define S_ISCHR(m) (((m) & S_IFMT) == S_IFCHR)
#define S_DAX (1 << 13)
#define IS_DAX(inode) ((inode)->i_flags & S_DAX)
#define I_DIRTY_SYNC (1 << 0)
#define I_DIRTY_DATASYNC (1 << 1)
#define I_DIRTY_PAGES (1 << 2)
#define I_FREEING (1 << 5)
#define IOCB_APPEND (1 << 4)
#define IOCB_NOWAIT (1 << 7)
#define IOCB_ATOMIC (1 << 9)
#define PAGECACHE_TAG_DIRTY XA_MARK_0
#define PAGECACHE_TAG_TOWRITE XA_MARK_1
#define SECTOR_SHIFT 9
#define READ 0
#define WRITE 1
#define VERIFY_READ READ
#define FALLOC_FL_KEEP_SIZE 0x01
#define FALLOC_FL_PUNCH_HOLE 0x02
#define FALLOC_FL_ZERO_RANGE 0x10
#define REMAP_FILE_DEDUP (1 << 0)
#define REMAP_FILE_ADVISORY (1 << 2)

struct kiocb;
struct iov_iter;
struct block_device;
struct readahead_control;
struct resource;

/* Named by linux/iomap.h, which fs/iomap/iter.c needs only the top of. */
struct bio {
	int unused;
};
struct sg_table {
	int unused;
};
typedef unsigned int blk_opf_t;

struct super_block {
	void *s_fs_info;
	loff_t s_maxbytes;
	unsigned long s_flags;
	/* A writeback's error, of any of its files, which the next sync returns, and clears: s_wb_err's. */
	int s_wb_err;
};

struct address_space {
	struct inode *host;
	struct xarray i_pages;
	struct rw_semaphore invalidate_lock;
	struct rw_semaphore i_mmap_rwsem;
	/* The VMAs mapping it, in the sim a list. */
	struct list_head i_mmap;
	unsigned long nrpages;
	const struct address_space_operations *a_ops;
	/* A writeback's error, which the next fsync returns, and clears: errseq_t's, and AS_EIO's. */
	int wb_err;
};

struct address_space_operations {
	int (*writepages)(struct address_space *mapping, struct writeback_control *wbc);
	bool (*dirty_folio)(struct address_space *mapping, struct folio *folio);
	ssize_t (*direct_IO)(struct kiocb *iocb, struct iov_iter *iter);
};

struct inode {
	umode_t i_mode;
	unsigned int i_flags;
	unsigned int i_nlink;
	u8 i_blkbits;
	unsigned long i_ino;
	loff_t i_size;
	u64 i_blocks;
	struct super_block *i_sb;
	struct address_space *i_mapping;
	const struct file_operations *i_fop;
	struct rw_semaphore i_rwsem;
	/* References: the mount's, and igrab's. */
	atomic_t i_count;
	unsigned int i_state;
	struct address_space i_data;
};

struct file {
	struct inode *f_inode;
	struct address_space *f_mapping;
	unsigned int f_flags;
};

struct kiocb {
	struct file *ki_filp;
	loff_t ki_pos;
	int ki_flags;
};

struct mnt_idmap;
struct iattr;
struct kvec {
	void *iov_base;
	size_t iov_len;
};
#define ITER_SOURCE WRITE

/* A read's destination or a write's source, in the kernel or at a task's simulated address. */
struct iov_iter {
	u8 data_source;
	bool user;
	size_t count;
	char *kbuf;
	unsigned long uaddr;
};

struct file_operations {
	int (*open)(struct inode *inode, struct file *file);
	int (*release)(struct inode *inode, struct file *file);
	loff_t (*llseek)(struct file *file, loff_t offset, int whence);
	ssize_t (*read_iter)(struct kiocb *iocb, struct iov_iter *to);
	ssize_t (*write_iter)(struct kiocb *iocb, struct iov_iter *from);
	int (*mmap)(struct file *file, struct vm_area_struct *vma);
	unsigned long (*get_unmapped_area)(struct file *file, unsigned long addr, unsigned long len,
					   unsigned long pgoff, unsigned long flags);
	ssize_t (*splice_read)(struct file *in, loff_t *ppos, void *pipe, size_t len,
			       unsigned int flags);
	ssize_t (*splice_write)(void *pipe, struct file *out, loff_t *ppos, size_t len,
				unsigned int flags);
	int (*fsync)(struct file *file, loff_t start, loff_t end, int datasync);
	long (*fallocate)(struct file *file, int mode, loff_t offset, loff_t len);
	loff_t (*remap_file_range)(struct file *file_in, loff_t pos_in, struct file *file_out,
				   loff_t pos_out, loff_t len, unsigned int remap_flags);
};

static inline struct inode *file_inode(const struct file *file) { return file->f_inode; }
static inline loff_t i_size_read(const struct inode *inode) { return READ_ONCE(inode->i_size); }
static inline void i_size_write(struct inode *inode, loff_t size) { WRITE_ONCE(inode->i_size, size); }
static inline unsigned int i_blocksize(const struct inode *inode) { return 1U << inode->i_blkbits; }
static inline bool sb_rdonly(const struct super_block *sb) { return false; }
static inline size_t iov_iter_count(const struct iov_iter *i) { return i->count; }
static inline bool iov_iter_is_kvec(const struct iov_iter *i) { return !i->user; }
static inline int iov_iter_rw(const struct iov_iter *i) { return i->data_source; }

#define inode_lock(inode) down_write(&(inode)->i_rwsem)
#define inode_unlock(inode) up_write(&(inode)->i_rwsem)
#define inode_lock_shared(inode) down_read(&(inode)->i_rwsem)
#define inode_unlock_shared(inode) up_read(&(inode)->i_rwsem)
#define inode_trylock_shared(inode) down_read_trylock(&(inode)->i_rwsem)
#define filemap_invalidate_lock(mapping) down_write(&(mapping)->invalidate_lock)
#define filemap_invalidate_unlock(mapping) up_write(&(mapping)->invalidate_lock)
#define filemap_invalidate_lock_shared(mapping) down_read(&(mapping)->invalidate_lock)
#define filemap_invalidate_unlock_shared(mapping) up_read(&(mapping)->invalidate_lock)
#define i_mmap_lock_read(mapping) down_read(&(mapping)->i_mmap_rwsem)
#define i_mmap_unlock_read(mapping) up_read(&(mapping)->i_mmap_rwsem)
#define sb_start_pagefault(sb) do { } while (0)
#define sb_end_pagefault(sb) do { } while (0)
#define file_accessed(file) do { } while (0)
#define file_update_time(file) 0
#define file_remove_privs(file) 0
#define mapping_gfp_mask(mapping) GFP_KERNEL
/* The file model's (kernel/shadow.c): a writeback of the mapping's file failed. */
void sim_shadow_wb_failed(struct address_space *mapping);
static inline void mapping_set_error(struct address_space *mapping, int error)
{
	if (!error)
		return;
	sim_shadow_wb_failed(mapping);
	mapping->wb_err = error;
	if (mapping->host)
		mapping->host->i_sb->s_wb_err = error;
}
static inline bool mapping_empty(const struct address_space *mapping)
{
	return xa_empty(&mapping->i_pages);
}
static inline bool mapping_tagged(struct address_space *mapping, xa_mark_t tag)
{
	return xa_marked(&mapping->i_pages, tag);
}

ssize_t generic_write_checks(struct kiocb *iocb, struct iov_iter *from);
ssize_t generic_write_sync(struct kiocb *iocb, ssize_t count);
void __mark_inode_dirty(struct inode *inode, int flags);
struct inode *igrab(struct inode *inode);
void iput(struct inode *inode);
void truncate_setsize(struct inode *inode, loff_t size);
int invalidate_inode_pages2_range(struct address_space *mapping, pgoff_t start, pgoff_t end);
int filemap_fdatawrite_range(struct address_space *mapping, loff_t start, loff_t end);
int filemap_write_and_wait_range(struct address_space *mapping, loff_t start, loff_t end);
void tag_pages_for_writeback(struct address_space *mapping, pgoff_t start, pgoff_t end);
int vfs_fsync_range(struct file *file, loff_t start, loff_t end, int datasync);
#define __generic_remap_file_range_prep(in, pos_in, out, pos_out, len, flags, ops) (-EOPNOTSUPP)
int generic_remap_file_range_prep(struct file *file_in, loff_t pos_in, struct file *file_out,
				  loff_t pos_out, loff_t *len, unsigned int remap_flags);
void lock_two_nondirectories(struct inode *inode1, struct inode *inode2);
void unlock_two_nondirectories(struct inode *inode1, struct inode *inode2);
void filemap_invalidate_lock_two(struct address_space *mapping1, struct address_space *mapping2);
void filemap_invalidate_unlock_two(struct address_space *mapping1,
				   struct address_space *mapping2);
#define inode_newsize_ok(inode, size) ((size) > (inode)->i_sb->s_maxbytes ? -EFBIG : 0)
#define file_modified(file) 0
void setattr_copy(struct mnt_idmap *idmap, struct inode *inode, const struct iattr *attr);
loff_t inode_get_bytes(struct inode *inode);
bool noop_dirty_folio(struct address_space *mapping, struct folio *folio);
ssize_t noop_direct_IO(struct kiocb *iocb, struct iov_iter *iter);
unsigned long thp_get_unmapped_area(struct file *file, unsigned long addr, unsigned long len,
				    unsigned long pgoff, unsigned long flags);
ssize_t copy_splice_read(struct file *in, loff_t *ppos, void *pipe, size_t len,
			 unsigned int flags);
ssize_t iter_file_splice_write(void *pipe, struct file *out, loff_t *ppos, size_t len,
			       unsigned int flags);
size_t iov_iter_zero(size_t bytes, struct iov_iter *i);
void iov_iter_kvec(struct iov_iter *i, unsigned int direction, const struct kvec *kvec,
		   unsigned long nr_segs, size_t count);
void iov_iter_revert(struct iov_iter *i, size_t bytes);
size_t copy_from_iter(void *addr, size_t bytes, struct iov_iter *i);
size_t fault_in_iov_iter_readable(const struct iov_iter *i, size_t bytes);
size_t fault_in_iov_iter_writeable(const struct iov_iter *i, size_t bytes);

enum writeback_sync_modes {
	WB_SYNC_NONE,
	WB_SYNC_ALL,
};
struct writeback_control {
	long nr_to_write;
	loff_t range_start, range_end;
	enum writeback_sync_modes sync_mode;
};

/* What dax.c shows of itself, to a test's buffer (sim_dax_stat), or, given none, the log. */
struct seq_file {
	char *buf;
	size_t size, count;
};
__printf(2, 3) void seq_printf(struct seq_file *m, const char *format, ...);
void seq_putc(struct seq_file *m, char c);

#endif
