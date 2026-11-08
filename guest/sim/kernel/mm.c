// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * Memory management beneath fs/dax.c: the window's struct pages, and each VMA's page tables, with
 * what mm/memory.c does to them for a DAX file's shared mapping, rewritten for two flat arrays:
 * inserting PTEs and PMDs and the checks that refuse them (insert_page_into_pte_locked, whose
 * refusals were TASK-124's SIGBUS), unmapping, write-protecting, and the fault paths that call a
 * mapping's vm_ops. A user access is one step of a task: no TLB, no store buffer. A pin, as
 * get_user_pages takes one, is a reference to the page alone. Private mappings aren't simulated.
 */

#include <sim/sim.h>
#include <sim/host.h>
#include <sim/shadow.h>
#include <linux/dax.h>

/*
 * Page frames: the huge zero page, then the window, each PMD-aligned, as dax_iomap_direct_access
 * has a PMD's pfn be. The small zero page is the huge one's first.
 */
#define SIM_ZERO_PFN (1UL << 20)
#define SIM_WINDOW_PFN (SIM_ZERO_PFN + (1UL << PMD_ORDER))

static struct page *window_pages;
static unsigned long window_pfns;
static char *window_memory;
static struct page zero_pages[1 << PMD_ORDER];
static char *zero_memory;

void sim_mm_init(void *window, u64 len)
{
	window_memory = window;
	window_pfns = len >> PAGE_SHIFT;
	window_pages = kzalloc(window_pfns * sizeof(*window_pages), GFP_KERNEL);
	zero_memory = kzalloc(PMD_SIZE, GFP_KERNEL);
	/* The zero page's reference, which nothing drops. */
	atomic_set(&zero_pages[0]._refcount, 1);
}

unsigned long sim_window_pfn(pgoff_t pgoff)
{
	return SIM_WINDOW_PFN + pgoff;
}

struct page *pfn_to_page(unsigned long pfn)
{
	if (pfn >= SIM_WINDOW_PFN && pfn < SIM_WINDOW_PFN + window_pfns)
		return &window_pages[pfn - SIM_WINDOW_PFN];
	if (pfn >= SIM_ZERO_PFN && pfn < SIM_WINDOW_PFN)
		return &zero_pages[pfn - SIM_ZERO_PFN];
	sim_panic("no page at pfn %lx", pfn);
}

unsigned long page_to_pfn(const struct page *page)
{
	if (page >= zero_pages && page < zero_pages + ARRAY_SIZE(zero_pages))
		return SIM_ZERO_PFN + (page - zero_pages);
	return SIM_WINDOW_PFN + (page - window_pages);
}

void *page_address(const struct page *page)
{
	unsigned long pfn = page_to_pfn(page);

	if (pfn < SIM_WINDOW_PFN)
		return zero_memory + ((pfn - SIM_ZERO_PFN) << PAGE_SHIFT);
	return window_memory + ((pfn - SIM_WINDOW_PFN) << PAGE_SHIFT);
}

unsigned long my_zero_pfn(unsigned long addr)
{
	return SIM_ZERO_PFN;
}

struct folio *mm_get_huge_zero_folio(struct mm_struct *mm)
{
	return (struct folio *)&zero_pages[0];
}

static bool is_zero_folio(const struct folio *folio)
{
	return &folio->page == &zero_pages[0];
}

/* Once a DAX page's last reference goes, it is idle, which a truncate may wait for. */
void folio_put(struct folio *folio)
{
	if (WARN_ON(atomic_read(&folio->_refcount) <= 0))
		return;
	if (atomic_dec_and_test(&folio->_refcount))
		wake_up_var(&folio->page);
}

void prep_compound_page(struct page *page, unsigned int order)
{
	unsigned long i;

	for (i = 1; i < 1UL << order; i++)
		page[i].compound_head = (unsigned long)page + 1;
	page->order = order;
}

vm_fault_t vmf_error(int err)
{
	if (err == -ENOMEM)
		return VM_FAULT_OOM;
	if (err == -EHWPOISON)
		return VM_FAULT_HWPOISON;
	return VM_FAULT_SIGBUS;
}

vm_fault_t finish_fault(struct vm_fault *vmf)
{
	sim_panic("a private mapping's fault: the sim makes only shared ones");
}

/*
 * A copy isn't one step: it reads its source before it writes its destination, and a store between
 * the two is lost where they overlap, as TASK-122's was: the page copied onto itself.
 */
unsigned long copy_mc_to_kernel(void *to, const void *from, unsigned long len)
{
	void *held = kmalloc(len, GFP_ATOMIC);

	memcpy(held, from, len);
	sim_point("a kernel copy");
	memcpy(to, held, len);
	kfree(held);
	return 0;
}

/* The user page whose PTE the log tells of, if any (sim_mm_watch). */
static unsigned long watched_addr = ~0UL;

void sim_mm_watch(unsigned long addr)
{
	watched_addr = addr & PAGE_MASK;
}

static void watch(unsigned long addr, const char *what, unsigned long pte)
{
	if (addr == watched_addr)
		sim_log("sim: address %lx: %s by %s: pte %lx, at %llu ns\n", addr, what, current->comm, pte,
			ktime_get_ns());
}

/* The tables' slots of `vma` for `addr`. */
static pte_t *pte_of(struct vm_area_struct *vma, unsigned long addr)
{
	return &vma->ptes[(addr - vma->vm_start) >> PAGE_SHIFT];
}

static pmd_t *pmd_of(struct vm_area_struct *vma, unsigned long addr)
{
	return &vma->pmds[(addr >> PMD_SHIFT) - (vma->vm_start >> PMD_SHIFT)];
}

/* As mm/memory.c's insert_page_into_pte_locked. */
vm_fault_t vmf_insert_page_mkwrite(struct vm_fault *vmf, struct page *page, bool write)
{
	struct vm_area_struct *vma = vmf->vma;
	struct folio *folio = page_folio(page);
	unsigned long addr = vmf->address, pfn = page_to_pfn(page);
	pte_t *pte, value;

	if (addr < vma->vm_start || addr >= vma->vm_end)
		return VM_FAULT_SIGBUS;
	/* validate_page_before_insert: a page with no reference, which no entry holds. */
	if (!folio_ref_count(folio))
		return VM_FAULT_SIGBUS;
	pte = pte_of(vma, addr);
	value = *pte;
	if (!pte_none(value)) {
		if (!write)
			return VM_FAULT_NOPAGE;
		if (pte_pfn(value) != pfn) {
			WARN_ON(!is_zero_pfn(pte_pfn(value)));
			return VM_FAULT_SIGBUS;
		}
		pte->pte |= SIM_PT_WRITE | SIM_PT_DIRTY;
		watch(addr, "made writable", pte->pte);
		return VM_FAULT_NOPAGE;
	}
	value.pte = (pfn << PAGE_SHIFT) | SIM_PT_PRESENT;
	if (!is_zero_folio(folio)) {
		folio_get(folio);
		atomic_inc(&folio->_mapcount);
		if (write)
			value.pte |= SIM_PT_WRITE | SIM_PT_DIRTY;
	}
	*pte = value;
	pmd_of(vma, addr)->pmd |= SIM_PT_TABLE;
	watch(addr, "inserted", value.pte);
	return VM_FAULT_NOPAGE;
}

vm_fault_t vmf_insert_folio_pmd(struct vm_fault *vmf, struct folio *folio, bool write)
{
	struct vm_area_struct *vma = vmf->vma;
	unsigned long addr = vmf->address & PMD_MASK;
	pmd_t *pmd = pmd_of(vma, addr);

	if (addr < vma->vm_start || addr + PMD_SIZE > vma->vm_end)
		return VM_FAULT_SIGBUS;
	if (WARN_ON_ONCE(folio_order(folio) != PMD_ORDER))
		return VM_FAULT_SIGBUS;
	if (!pmd_none(*pmd)) {
		if (!write)
			return VM_FAULT_NOPAGE;
		if (pmd_pfn(*pmd) != folio_pfn(folio)) {
			WARN_ON(!is_zero_pfn(pmd_pfn(*pmd)));
			return VM_FAULT_SIGBUS;
		}
		pmd->pmd |= SIM_PT_WRITE | SIM_PT_DIRTY;
		return VM_FAULT_NOPAGE;
	}
	if (!is_zero_folio(folio)) {
		folio_get(folio);
		atomic_inc(&folio->_mapcount);
	}
	pmd->pmd = (folio_pfn(folio) << PAGE_SHIFT) | SIM_PT_PRESENT | SIM_PT_HUGE |
		   (write ? SIM_PT_WRITE | SIM_PT_DIRTY : 0);
	return VM_FAULT_NOPAGE;
}

/*
 * A mapping gone, with the reference it held: zap_present_ptes's. Its PTE or PMD is clear already,
 * as the kernel's page table lock has it: a put may wake a task, and so switch to one.
 */
static void drop_mapping(unsigned long pfn)
{
	struct folio *folio = pfn_folio(pfn);

	if (is_zero_folio(folio))
		return;
	WARN_ON(atomic_read(&folio->_mapcount) <= 0);
	atomic_dec(&folio->_mapcount);
	folio_put(folio);
}

static void zap_pmd(pmd_t *pmd)
{
	pmd_t value = *pmd;

	if (pmd_trans_huge(value)) {
		pmd->pmd = 0;
		drop_mapping(pmd_pfn(value));
	}
}

/* The index in `vma`'s range of its page at `addr`. */
static pgoff_t page_in(struct vm_area_struct *vma, unsigned long addr)
{
	return (addr - vma->vm_start) >> PAGE_SHIFT;
}

/*
 * The pages of `vma` from `from` to `to`, held or not: as a fault maps them, or as something other
 * than a fault unmaps them. Faults again on a page held, a load's, are a fault's unmapping of it:
 * read faults undoing each other's mappings, a fault each, which nothing else checks, as each
 * succeeds (TASK-152, the livelock patch 0010 ends).
 */
static void set_held(struct vm_area_struct *vma, unsigned long from, unsigned long to, bool held)
{
	from = max(from, vma->vm_start);
	to = min(to, vma->vm_end);
	for (; from < to; from += PAGE_SIZE)
		vma->held[page_in(vma, from)] = held;
}

/* What `vma` maps at `addr`, held: its page, or its PMD's row. */
static void hold(struct vm_area_struct *vma, unsigned long addr)
{
	if (pmd_trans_huge(*pmd_of(vma, addr)))
		set_held(vma, addr & PMD_MASK, (addr & PMD_MASK) + PMD_SIZE, true);
	else
		set_held(vma, addr & PAGE_MASK, (addr & PAGE_MASK) + PAGE_SIZE, true);
}

/*
 * Whether the current task is in a read fault whose page is among `nr` of `mapping` at `start`: its
 * own page unmapped, from every mapping, as a read fault replaces its entry (dax_insert_entry,
 * which patch 0010 stops for a page the entry names), which holds what it unmaps still. A fault's
 * other unmappings are as anything else's: of other ranges, as one evicts a range to make room in
 * the window (fxfs_dax_release), and a store's of its own, as it drops the slice it can't store to
 * (fxfs_dax_fault_drop_slice), an UNMAP of the range.
 */
static bool faults_on(struct address_space *mapping, pgoff_t start, pgoff_t nr)
{
	pgoff_t last = start + nr - 1;

	if (last < start)
		last = ULONG_MAX;
	return current->fault_drops >= 0 && !current->fault_write &&
	       current->fault_mapping == mapping && current->fault_pgoff >= start &&
	       current->fault_pgoff <= last;
}

/*
 * How file THP splits: the PMD goes, and faults map the PTEs again: what a shared PMD's write
 * fault does by design (wp_huge_pmd), the row's pages let go of, as they fault again once.
 */
void split_huge_pmd(struct vm_area_struct *vma, pmd_t *pmd, unsigned long address)
{
	zap_pmd(pmd);
	set_held(vma, address & PMD_MASK, (address & PMD_MASK) + PMD_SIZE, false);
}

/* Each VMA of `mapping` overlapping `nr` pages at `start`, as addresses `from` to `to`. */
#define for_each_mapped(vma, mapping, start, nr, from, to)				\
	list_for_each_entry(vma, &(mapping)->i_mmap, mapping_list)			\
		if (sim_overlap(vma, start, nr, &(from), &(to)))

static bool sim_overlap(struct vm_area_struct *vma, pgoff_t start, pgoff_t nr, unsigned long *from,
			unsigned long *to)
{
	pgoff_t pages = (vma->vm_end - vma->vm_start) >> PAGE_SHIFT;
	pgoff_t first = max(start, vma->vm_pgoff);
	/* As unmap_mapping_pages: a range running off the end, even to 0 pages, goes to the end. */
	pgoff_t last = start + nr - 1;

	if (last < start)
		last = ULONG_MAX;
	last = min(last, vma->vm_pgoff + pages - 1);
	if (first > last)
		return false;
	*from = vma->vm_start + ((first - vma->vm_pgoff) << PAGE_SHIFT);
	*to = vma->vm_start + ((last + 1 - vma->vm_pgoff) << PAGE_SHIFT);
	return true;
}

void unmap_mapping_pages(struct address_space *mapping, pgoff_t start, pgoff_t nr, bool even_cows)
{
	struct vm_area_struct *vma;
	unsigned long from, to, addr;

	i_mmap_lock_read(mapping);
	for_each_mapped(vma, mapping, start, nr, from, to) {
		/* Not a fault's own page: a rebind's MAP, an UNMAP, a truncate, a punch, an eviction. */
		if (!faults_on(mapping, start, nr))
			set_held(vma, from, to, false);
		for (addr = from; addr < to; addr += PAGE_SIZE) {
			pte_t *pte = pte_of(vma, addr), value = *pte;

			zap_pmd(pmd_of(vma, addr));
			if (pte_none(value))
				continue;
			pte->pte = 0;
			watch(addr, "zapped", value.pte);
			drop_mapping(pte_pfn(value));
		}
	}
	i_mmap_unlock_read(mapping);
}

int walk_page_mapping(struct address_space *mapping, pgoff_t first_index, pgoff_t nr,
		      const struct mm_walk_ops *ops, void *private)
{
	struct mm_walk walk = { .ops = ops, .private = private };
	struct vm_area_struct *vma;
	unsigned long from, to, addr;
	int err;

	for_each_mapped(vma, mapping, first_index, nr, from, to) {
		walk.vma = vma;
		for (addr = from; addr < to; addr += PAGE_SIZE) {
			err = ops->pte_entry(pte_of(vma, addr), addr, addr + PAGE_SIZE, &walk);
			if (err)
				return err;
		}
	}
	return 0;
}

/* As mm/mapping_dirty_helpers.c's: a writable PMD can't be write-protected, and WARNs. */
unsigned long wp_shared_mapping_range(struct address_space *mapping, pgoff_t first_index,
				      pgoff_t nr)
{
	struct vm_area_struct *vma;
	unsigned long from, to, addr, total = 0;

	i_mmap_lock_read(mapping);
	for_each_mapped(vma, mapping, first_index, nr, from, to) {
		for (addr = from; addr < to; addr += PAGE_SIZE) {
			pte_t *pte = pte_of(vma, addr);

			WARN_ON(pmd_write(*pmd_of(vma, addr)));
			if (pte_write(*pte)) {
				pte->pte &= ~SIM_PT_WRITE;
				watch(addr, "write-protected", pte->pte);
				total++;
			}
		}
	}
	i_mmap_unlock_read(mapping);
	return total;
}

int pfn_mkclean_range(unsigned long pfn, unsigned long nr_pages, pgoff_t pgoff,
		      struct vm_area_struct *vma)
{
	unsigned long i, addr;
	int cleaned = 0;

	for (i = 0; i < nr_pages; i++) {
		pgoff_t at = pgoff + i;
		pte_t *pte;

		if (at < vma->vm_pgoff ||
		    at >= vma->vm_pgoff + ((vma->vm_end - vma->vm_start) >> PAGE_SHIFT))
			continue;
		addr = vma->vm_start + ((at - vma->vm_pgoff) << PAGE_SHIFT);
		pte = pte_of(vma, addr);
		if (pte_present(*pte) && pte_pfn(*pte) == pfn + i &&
		    (pte->pte & (SIM_PT_WRITE | SIM_PT_DIRTY))) {
			pte->pte &= ~(SIM_PT_WRITE | SIM_PT_DIRTY);
			watch(addr, "cleaned", pte->pte);
			cleaned++;
		}
	}
	return cleaned;
}

/*
 * A user's PTE maps a window page writably, of `pages` from `page`: what a store could hit, with
 * no fault, however the host maps the page. The address, or 0.
 */
unsigned long sim_mm_writable_mapping(u64 page, u64 pages)
{
	struct vm_area_struct *vma;
	unsigned long i, count, pfn;

	list_for_each_entry(vma, &current->mm->vmas, mm_list) {
		count = (vma->vm_end - vma->vm_start) >> PAGE_SHIFT;
		for (i = 0; i < count; i++) {
			if (!pte_write(vma->ptes[i]))
				continue;
			pfn = pte_pfn(vma->ptes[i]);
			if (pfn >= sim_window_pfn(page) && pfn < sim_window_pfn(page + pages))
				return vma->vm_start + (i << PAGE_SHIFT);
		}
	}
	return 0;
}

static struct vm_area_struct *find_vma(struct mm_struct *mm, unsigned long addr)
{
	struct vm_area_struct *vma;

	list_for_each_entry(vma, &mm->vmas, mm_list) {
		if (addr >= vma->vm_start && addr < vma->vm_end)
			return vma;
	}
	return NULL;
}

/*
 * __handle_mm_fault, for a shared mapping of a DAX file: a user access's (`user`), or the kernel's,
 * as get_user_pages faults a page in.
 */
static vm_fault_t handle_fault(struct vm_area_struct *vma, unsigned long addr, bool write, bool user)
{
	struct vm_fault vmf = {
		.vma = vma,
		.address = addr & PAGE_MASK,
		.pgoff = linear_page_index(vma, addr),
		.flags = (user ? FAULT_FLAG_USER : 0) | (write ? FAULT_FLAG_WRITE : 0),
		.pmd = pmd_of(vma, addr),
	};
	pte_t *pte = pte_of(vma, addr);
	vm_fault_t ret;

	current->fault_mapping = vma->vm_file->f_mapping;
	current->fault_pgoff = vmf.pgoff;
	current->fault_write = write;
	if (pmd_none(*vmf.pmd) && (vma->vm_flags & VM_HUGEPAGE)) {
		ret = vma->vm_ops->huge_fault(&vmf, PMD_ORDER);
		if (!(ret & VM_FAULT_FALLBACK))
			return ret;
	}
	if (pmd_trans_huge(*vmf.pmd)) {
		if (!write || pmd_write(*vmf.pmd))
			return 0;
		/* wp_huge_pmd: a shared one falls back to PTEs here, and the PMD splits. */
		ret = vma->vm_ops->huge_fault(&vmf, PMD_ORDER);
		if (!(ret & VM_FAULT_FALLBACK))
			return ret;
		split_huge_pmd(vma, vmf.pmd, addr);
	}
	if (pte_none(*pte))
		return vma->vm_ops->fault(&vmf);
	if (write && !pte_write(*pte)) {
		/* do_wp_page: a shared mapping's, which FS DAX has told of (wp_pfn_shared). */
		vmf.flags |= FAULT_FLAG_MKWRITE;
		ret = vma->vm_ops->pfn_mkwrite(&vmf);
		if (ret & (VM_FAULT_ERROR | VM_FAULT_NOPAGE))
			return ret;
		pte->pte |= SIM_PT_WRITE | SIM_PT_DIRTY;
	}
	return 0;
}

/* The page a user access of `addr` reaches, or NULL if it would fault. */
static struct page *translate_page(struct vm_area_struct *vma, unsigned long addr, bool write)
{
	pmd_t pmd = *pmd_of(vma, addr);
	pte_t pte = *pte_of(vma, addr);

	if (pmd_trans_huge(pmd)) {
		if (write && !pmd_write(pmd))
			return NULL;
		return pfn_to_page(pmd_pfn(pmd) + ((addr >> PAGE_SHIFT) & (PTRS_PER_PTE - 1)));
	}
	if (!pte_present(pte) || (write && !pte_write(pte)))
		return NULL;
	return pfn_to_page(pte_pfn(pte));
}

/* What a user access of `addr` reaches, or NULL if it would fault. */
static void *translate(struct vm_area_struct *vma, unsigned long addr, bool write)
{
	struct page *page = translate_page(vma, addr, write);

	return page ? page_address(page) + offset_in_page(addr) : NULL;
}

unsigned long sim_mm_faults;

size_t sim_copy_user(unsigned long addr, void *buf, size_t len, bool write)
{
	size_t done = 0;

	while (done < len) {
		struct vm_area_struct *vma = find_vma(current->mm, addr);
		size_t part = min_t(size_t, PAGE_SIZE - offset_in_page(addr), len - done);
		void *at;

		if (!vma)
			break;
		at = translate(vma, addr, write);
		if (!at) {
			struct mm_struct *mm = current->mm;
			vm_fault_t ret = VM_FAULT_NOPAGE;

			if (pagefault_disabled())
				break;
			sim_mm_faults++;
			/* The kernel's mmap_lock: a fault keeps its VMA, as a task unmaps it. */
			down_read(&mm->lock);
			vma = find_vma(mm, addr);
			/* Unmapped still, under the lock: another task's fault may have mapped it. */
			if (vma && !write && vma->held[page_in(vma, addr)] && !translate(vma, addr, false))
				sim_panic("a load faults again: page %lu of a mapping at %lx, which held it, "
					  "and nothing but a fault unmapped it", page_in(vma, addr),
					  vma->vm_start);
			current->fault_drops = 0;
			if (vma)
				ret = handle_fault(vma, addr, write, true);
			current->fault_drops = -1;
			if (vma && translate_page(vma, addr, false))
				hold(vma, addr);
			up_read(&mm->lock);
			if (!vma || (ret & VM_FAULT_ERROR))
				break;
			continue;
		}
		if (write)
			memcpy(at, buf + done, part);
		else
			memcpy(buf + done, at, part);
		addr += part;
		done += part;
	}
	return done;
}

void sim_mm_describe(unsigned long addr)
{
	struct vm_area_struct *vma = find_vma(current->mm, addr);
	pte_t pte = *pte_of(vma, addr);
	pmd_t pmd = *pmd_of(vma, addr);
	unsigned long pfn = pmd_trans_huge(pmd) ?
			    pmd_pfn(pmd) + ((addr >> PAGE_SHIFT) & (PTRS_PER_PTE - 1)) : pte_pfn(pte);

	sim_log("sim: address %lx: pte %lx, pmd %lx, pfn %lx: ", addr, pte.pte, pmd.pmd, pfn);
	if (pfn >= SIM_WINDOW_PFN)
		sim_device_describe(pfn - SIM_WINDOW_PFN);
	else
		sim_log("the zero page\n");
}

bool sim_mm_is_huge(unsigned long addr)
{
	return pmd_trans_huge(*pmd_of(find_vma(current->mm, addr), addr));
}

/*
 * pin_user_pages' of the page at `addr`, read-only, as vmsplice's: faulted in by the kernel if
 * need be, which isn't a user fault, and held by a reference, which keeps the page busy
 * (dax_layout_busy_page_range) until sim_unpin_page. The page, or NULL if the fault failed.
 */
struct page *sim_pin_user_page(unsigned long addr)
{
	struct mm_struct *mm = current->mm;
	struct vm_area_struct *vma;
	struct page *page = NULL;
	vm_fault_t ret = 0;

	down_read(&mm->lock);
	for (;;) {
		vma = find_vma(mm, addr);
		if (!vma || (ret & VM_FAULT_ERROR))
			break;
		page = translate_page(vma, addr, false);
		if (page) {
			folio_get(page_folio(page));
			break;
		}
		sim_mm_faults++;
		/* Unmapped, under the lock, as translate_page above found it. */
		if (vma->held[page_in(vma, addr)])
			sim_panic("a pin faults again: page %lu of a mapping at %lx, which held it, and "
				  "nothing but a fault unmapped it", page_in(vma, addr), vma->vm_start);
		current->fault_drops = 0;
		ret = handle_fault(vma, addr, false, false);
		current->fault_drops = -1;
		if (translate_page(vma, addr, false))
			hold(vma, addr);
	}
	up_read(&mm->lock);
	return page;
}

/* unpin_user_page: once the pin was the page's last use, the page is idle. */
void sim_unpin_page(struct page *page)
{
	folio_put(page_folio(page));
}

/* Its pieces, a mapping's each, the file model's calls (kernel/shadow.c). */
void sim_user_access_begin(struct sim_user_access *access, unsigned long addr, size_t len,
			   bool store)
{
	size_t at = 0;

	access->count = 0;
	access->store = store;
	while (at < len && access->count < SIM_ACCESS_PIECES) {
		struct vm_area_struct *vma = find_vma(current->mm, addr + at);
		size_t part;
		loff_t pos;

		if (!vma)
			break;
		part = min_t(size_t, len - at, vma->vm_end - (addr + at));
		pos = ((loff_t)vma->vm_pgoff << PAGE_SHIFT) + (addr + at - vma->vm_start);
		if (store)
			sim_shadow_store_begin(&access->pieces[access->count].call, vma->vm_file, pos,
					       part);
		else
			sim_shadow_read_begin(&access->pieces[access->count].call, vma->vm_file, pos,
					      part);
		access->pieces[access->count].at = at;
		access->pieces[access->count].len = part;
		access->count++;
		at += part;
	}
}

void sim_user_access_end(struct sim_user_access *access, const void *bytes, size_t done)
{
	int i;

	for (i = 0; i < access->count; i++) {
		size_t at = access->pieces[i].at;
		size_t got = done > at ? min(done - at, access->pieces[i].len) : 0;

		if (access->store)
			sim_shadow_store_end(&access->pieces[i].call, bytes ? bytes + at : NULL, got);
		else
			sim_shadow_read_end(&access->pieces[i].call, "a load", bytes + at, got);
	}
}

int sim_user_store(unsigned long addr, const void *buf, size_t len)
{
	struct sim_user_access access;
	size_t done;

	sim_user_access_begin(&access, addr, len, true);
	done = sim_copy_user(addr, (void *)buf, len, true);
	sim_user_access_end(&access, buf, done);
	return done == len ? 0 : -EFAULT;
}

int sim_user_load(unsigned long addr, void *buf, size_t len)
{
	struct sim_user_access access;
	size_t done;

	sim_user_access_begin(&access, addr, len, false);
	done = sim_copy_user(addr, buf, len, false);
	sim_user_access_end(&access, buf, done);
	return done == len ? 0 : -EFAULT;
}

/* A VMA's page tables go with it. */
unsigned long sim_mm_map(struct mm_struct *mm, struct file *file, pgoff_t pgoff, size_t len)
{
	struct address_space *mapping = file->f_mapping;
	struct vm_area_struct *vma = kzalloc(sizeof(*vma), GFP_KERNEL);
	unsigned long pmds;
	int err;

	/* At the PMD alignment of its offset, as thp_get_unmapped_area keeps it. */
	vma->vm_start = ALIGN(mm->next, PMD_SIZE) + ((pgoff << PAGE_SHIFT) & ~PMD_MASK);
	vma->vm_end = vma->vm_start + PAGE_ALIGN(len);
	mm->next = vma->vm_end + PMD_SIZE;
	vma->vm_pgoff = pgoff;
	vma->vm_flags = VM_READ | VM_WRITE | VM_SHARED | VM_MAYSHARE;
	vma->vm_file = file;
	vma->vm_mm = mm;
	vma->ptes = kcalloc(PAGE_ALIGN(len) >> PAGE_SHIFT, sizeof(*vma->ptes), GFP_KERNEL);
	vma->held = kcalloc(PAGE_ALIGN(len) >> PAGE_SHIFT, sizeof(*vma->held), GFP_KERNEL);
	pmds = (vma->vm_end >> PMD_SHIFT) - (vma->vm_start >> PMD_SHIFT) + 1;
	vma->pmds = kcalloc(pmds, sizeof(*vma->pmds), GFP_KERNEL);
	err = file->f_inode->i_fop->mmap(file, vma);
	if (err)
		sim_panic("mmap: %d", err);
	down_write(&mm->lock);
	list_add_tail(&vma->mm_list, &mm->vmas);
	down_write(&mapping->i_mmap_rwsem);
	list_add_tail(&vma->mapping_list, &mapping->i_mmap);
	up_write(&mapping->i_mmap_rwsem);
	up_write(&mm->lock);
	return vma->vm_start;
}

void sim_munmap(unsigned long addr)
{
	struct mm_struct *mm = current->mm;
	struct vm_area_struct *vma;
	struct address_space *mapping;

	down_write(&mm->lock);
	vma = find_vma(mm, addr);
	mapping = vma->vm_file->f_mapping;
	unmap_mapping_pages(mapping, vma->vm_pgoff,
			    (vma->vm_end - vma->vm_start) >> PAGE_SHIFT, false);
	down_write(&mapping->i_mmap_rwsem);
	list_del(&vma->mapping_list);
	up_write(&mapping->i_mmap_rwsem);
	list_del(&vma->mm_list);
	up_write(&mm->lock);
	kfree(vma->ptes);
	kfree(vma->held);
	kfree(vma->pmds);
	kfree(vma);
}
