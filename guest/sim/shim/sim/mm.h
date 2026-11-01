/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * Memory management, faked at the level fs/dax.c meets it: pages of the window, with the
 * references and map counts it reads; and a mapping's page tables, a PTE and a PMD array in each
 * VMA, which only the sim's user accesses read (kernel/mm.c). There is no TLB and no MMU behind
 * them: a user store looks its PTE up and is done, as one step.
 */
#ifndef _SIM_MM_H
#define _SIM_MM_H

#include <sim/slab.h>
#include <sim/bitops.h>
#include <sim/rcu.h>
#include <linux/list.h>

#define PAGE_SHIFT 12
#define PAGE_SIZE (1UL << PAGE_SHIFT)
#define PAGE_MASK (~(PAGE_SIZE - 1))
#define PMD_SHIFT 21
#define PMD_SIZE (1UL << PMD_SHIFT)
#define PMD_MASK (~(PMD_SIZE - 1))
#define PMD_ORDER (PMD_SHIFT - PAGE_SHIFT)
#define PTRS_PER_PTE (1UL << PMD_ORDER)
#define PAGE_ALIGNED(addr) IS_ALIGNED((unsigned long)(addr), PAGE_SIZE)
#define PAGE_ALIGN(addr) ALIGN(addr, PAGE_SIZE)
#define offset_in_page(p) ((unsigned long)(p) & ~PAGE_MASK)
#define PFN_PHYS(x) ((phys_addr_t)(x) << PAGE_SHIFT)
#define PHYS_PFN(x) ((unsigned long)((x) >> PAGE_SHIFT))

struct address_space;
struct dev_pagemap;
struct mm_struct;
struct vm_area_struct;
struct file;
struct inode;
struct writeback_control;

#define __SIM_PAGE_FIELDS							\
	unsigned long flags;							\
	/* The head of its compound page, with bit 0 set, or 0. */		\
	unsigned long compound_head;						\
	struct address_space *mapping;						\
	union {									\
		pgoff_t index;							\
		unsigned long share;						\
	};									\
	struct dev_pagemap *pgmap;						\
	atomic_t _refcount;							\
	/* How many PTEs and PMDs map it, in its head. */			\
	atomic_t _mapcount;							\
	/* Its compound page's order, in its head. */				\
	unsigned int order;							\
	struct list_head _deferred_list;

struct page {
	__SIM_PAGE_FIELDS
};

struct folio {
	union {
		struct {
			__SIM_PAGE_FIELDS
		};
		struct page page;
	};
};

/* Physical page frames: the two zero pages' and the window's (kernel/mm.c). */
struct page *pfn_to_page(unsigned long pfn);
unsigned long page_to_pfn(const struct page *page);
void *page_address(const struct page *page);
#define pfn_folio(pfn) page_folio(pfn_to_page(pfn))
unsigned long my_zero_pfn(unsigned long addr);
#define is_zero_pfn(pfn) ((pfn) == my_zero_pfn(0))

static inline struct folio *page_folio(const struct page *page)
{
	unsigned long head = READ_ONCE(page->compound_head);

	return (struct folio *)(head & 1 ? head - 1 : (unsigned long)page);
}
#define page_pgmap(page) ((page)->pgmap)
#define folio_page(folio, n) (&(folio)->page + (n))
#define folio_ref_count(folio) atomic_read(&(folio)->_refcount)
#define folio_ref_inc(folio) atomic_inc(&(folio)->_refcount)
#define folio_get(folio) atomic_inc(&(folio)->_refcount)
#define folio_order(folio) ((folio)->order)
#define folio_reset_order(folio) ((folio)->order = 0)
#define folio_pfn(folio) page_to_pfn(&(folio)->page)
#define folio_mapcount(folio) atomic_read(&(folio)->_mapcount)
#define page_ref_count(page) folio_ref_count((struct folio *)(page))
void folio_put(struct folio *folio);
void prep_compound_page(struct page *page, unsigned int order);
#define clear_compound_head(page) ((page)->compound_head = 0)
#define ClearPageHead(page) do { } while (0)
#define __SetPageUptodate(page) do { } while (0)

/* A PTE or PMD: its page frame, shifted, and these bits. */
typedef struct { unsigned long pte; } pte_t;
typedef struct { unsigned long pmd; } pmd_t;
typedef struct { unsigned long pgprot; } pgprot_t;

#define SIM_PT_PRESENT 1UL
#define SIM_PT_WRITE 2UL
#define SIM_PT_DIRTY 4UL
#define SIM_PT_HUGE 8UL
/* A PMD with a PTE table under it, which stays, so that it is not none, as zaps leave it. */
#define SIM_PT_TABLE 16UL

static inline bool pte_none(pte_t pte) { return !pte.pte; }
static inline bool pte_present(pte_t pte) { return pte.pte & SIM_PT_PRESENT; }
static inline bool pte_write(pte_t pte) { return pte.pte & SIM_PT_WRITE; }
static inline bool pte_dirty(pte_t pte) { return pte.pte & SIM_PT_DIRTY; }
static inline unsigned long pte_pfn(pte_t pte) { return pte.pte >> PAGE_SHIFT; }
#define ptep_get(ptep) (*(ptep))
static inline bool pmd_none(pmd_t pmd) { return !pmd.pmd; }
static inline bool pmd_trans_huge(pmd_t pmd) { return pmd.pmd & SIM_PT_HUGE; }
static inline bool pmd_write(pmd_t pmd) { return pmd.pmd & SIM_PT_WRITE; }
static inline unsigned long pmd_pfn(pmd_t pmd) { return pmd.pmd >> PAGE_SHIFT; }

typedef unsigned long vm_flags_t;

#define VM_READ 0x00000001UL
#define VM_WRITE 0x00000002UL
#define VM_SHARED 0x00000008UL
#define VM_MAYSHARE 0x00000080UL
#define VM_MIXEDMAP 0x10000000UL
#define VM_HUGEPAGE 0x20000000UL
#define VM_SYNC 0x00800000UL

enum fault_flag {
	FAULT_FLAG_WRITE = 1 << 0,
	FAULT_FLAG_MKWRITE = 1 << 1,
	FAULT_FLAG_USER = 1 << 6,
};

#define VM_FAULT_OOM 0x000001U
#define VM_FAULT_SIGBUS 0x000002U
#define VM_FAULT_MAJOR 0x000004U
#define VM_FAULT_HWPOISON 0x000010U
#define VM_FAULT_SIGSEGV 0x000040U
#define VM_FAULT_NOPAGE 0x000100U
#define VM_FAULT_LOCKED 0x000200U
#define VM_FAULT_RETRY 0x000400U
#define VM_FAULT_FALLBACK 0x000800U
#define VM_FAULT_DONE_COW 0x001000U
#define VM_FAULT_NEEDDSYNC 0x002000U
#define VM_FAULT_ERROR (VM_FAULT_OOM | VM_FAULT_SIGBUS | VM_FAULT_SIGSEGV | VM_FAULT_HWPOISON)

struct vm_fault {
	struct vm_area_struct *vma;
	pgoff_t pgoff;
	unsigned long address;
	unsigned int flags;
	pmd_t *pmd;
	/* A private mapping's copy. The sim maps only shared: always NULL. */
	struct page *cow_page;
};

struct vm_operations_struct {
	vm_fault_t (*fault)(struct vm_fault *vmf);
	vm_fault_t (*huge_fault)(struct vm_fault *vmf, unsigned int order);
	vm_fault_t (*page_mkwrite)(struct vm_fault *vmf);
	vm_fault_t (*pfn_mkwrite)(struct vm_fault *vmf);
};

struct mm_struct {
	struct list_head vmas;
	/* The next address a mapping takes. */
	unsigned long next;
};

struct vm_area_struct {
	unsigned long vm_start, vm_end;
	unsigned long vm_flags;
	unsigned long vm_pgoff;
	struct file *vm_file;
	const struct vm_operations_struct *vm_ops;
	struct mm_struct *vm_mm;
	/* Its page tables, by page and by PMD of its range. */
	pte_t *ptes;
	pmd_t *pmds;
	/* On its mm's, and on its file's mapping's (`i_mmap`). */
	struct list_head mm_list, mapping_list;
};

static inline void vm_flags_set(struct vm_area_struct *vma, unsigned long flags)
{
	vma->vm_flags |= flags;
}
static inline pgoff_t linear_page_index(const struct vm_area_struct *vma, unsigned long address)
{
	return ((address - vma->vm_start) >> PAGE_SHIFT) + vma->vm_pgoff;
}

/* Inserts what fs/dax.c asks into the page tables, as mm/memory.c's. */
vm_fault_t vmf_insert_page_mkwrite(struct vm_fault *vmf, struct page *page, bool write);
vm_fault_t vmf_insert_folio_pmd(struct vm_fault *vmf, struct folio *folio, bool write);
void unmap_mapping_pages(struct address_space *mapping, pgoff_t start, pgoff_t nr, bool even_cows);
void split_huge_pmd(struct vm_area_struct *vma, pmd_t *pmd, unsigned long address);
int pfn_mkclean_range(unsigned long pfn, unsigned long nr_pages, pgoff_t pgoff,
		      struct vm_area_struct *vma);
unsigned long wp_shared_mapping_range(struct address_space *mapping, pgoff_t first_index,
				      pgoff_t nr);
#define vma_interval_tree_foreach(vma, root, start, last)			\
	list_for_each_entry(vma, root, mapping_list)				\
		if (vma->vm_pgoff <= (last) &&					\
		    vma->vm_pgoff + ((vma->vm_end - vma->vm_start) >> PAGE_SHIFT) > (start))

enum page_walk_lock {
	PGWALK_RDLOCK,
};

struct mm_walk;
struct mm_walk_ops {
	int (*pte_entry)(pte_t *pte, unsigned long addr, unsigned long next, struct mm_walk *walk);
	enum page_walk_lock walk_lock;
};
struct mm_walk {
	const struct mm_walk_ops *ops;
	struct vm_area_struct *vma;
	void *private;
};
int walk_page_mapping(struct address_space *mapping, pgoff_t first_index, pgoff_t nr,
		      const struct mm_walk_ops *ops, void *private);

/* What fs/dax.c counts and bills: nothing here. */
#define PGMAJFAULT 0
#define THP_FAULT_FALLBACK 0
#define count_vm_event(event) do { } while (0)
#define count_memcg_event_mm(mm, event) do { } while (0)

/* What private mappings would need, which the sim doesn't make. */
#define kmap_atomic(page) page_address(page)
#define kunmap_atomic(addr) do { } while (0)
#define copy_user_page(to, from, addr, page) memcpy(to, from, PAGE_SIZE)
#define clear_user_highpage(page, addr) memset(page_address(page), 0, PAGE_SIZE)
vm_fault_t finish_fault(struct vm_fault *vmf);

struct folio *mm_get_huge_zero_folio(struct mm_struct *mm);
vm_fault_t vmf_error(int err);
unsigned long copy_mc_to_kernel(void *to, const void *from, unsigned long len);

#endif
