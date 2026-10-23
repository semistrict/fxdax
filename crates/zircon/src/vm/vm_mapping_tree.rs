// Copyright 2025 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The mappings of a VMO, in a tree by the offsets they map from, which finds those of a range.
//!
//! Ported from `zircon/kernel/vm/include/vm/vm_mapping_observer.h`, the `MappingTree` of
//! `zircon/kernel/vm/include/vm/vm_object.h`, and the walk of
//! `VmObject::RangeChangeUpdateMappingsLocked` in `zircon/kernel/vm/vm_object.cc`.
//!
//! A VMO's mappings may overlap, and many may map the same offset. They are kept in order of the
//! offset each maps from, and, among those of an offset, of the mapping's address, and each node
//! of the tree keeps the greatest end of the mappings below it. Finding the mappings of a range
//! then skips each subtree whose mappings all end at or before the range starts, or start at or
//! after it ends, rather than look at every mapping of the VMO.
//!
//! Deviations:
//! - fxdax: an AVL tree of its own, rather than Zircon's B-tree (`zircon/kernel/lib/btree`)
//!   augmented by `VmMappingObserver`: std's `BTreeMap`, which stands in for that B-tree elsewhere
//!   (`vm_page_list_btree`), cannot keep what is below each of its nodes. A node keeps only the
//!   greatest end below it, as the least offset below it is its leftmost descendant's, and none to
//!   its right starts before it.
//! - Only the mappings that map part of the range are visited, as each node of the tree is one
//!   mapping: Zircon's walk visits all those of a B-tree leaf any of which might.

use std::cmp::Ordering;

/// A mapping's place among its VMO's: the offset it maps from, then, among those of an offset, its
/// address, which is its own while it lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct MappingKey {
    pub object_offset: u64,
    pub id: usize,
}

/// A VMO's mappings, each of a range of it, by their keys.
pub(crate) struct MappingTree<T> {
    root: Link<T>,
    len: usize,
}

type Link<T> = Option<Box<Node<T>>>;

struct Node<T> {
    key: MappingKey,
    /// The end of the range of the VMO it maps.
    end: u64,
    /// The greatest end of its own and its descendants'.
    max_end: u64,
    /// The height of the subtree it is the root of, a leaf's 1.
    height: u8,
    left: Link<T>,
    right: Link<T>,
    value: T,
}

impl<T> MappingTree<T> {
    pub const fn new() -> Self {
        Self { root: None, len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Adds `value`, which maps `len` bytes from `key.object_offset`, and returns whether it was
    /// added: not if `key` is already there.
    pub fn insert(&mut self, key: MappingKey, len: u64, value: T) -> bool {
        debug_assert!(len != 0);
        let end = key.object_offset + len;
        let node = Node { key, end, max_end: end, height: 1, left: None, right: None, value };
        let inserted = insert(&mut self.root, Box::new(node)).is_some();
        self.len += usize::from(inserted);
        inserted
    }

    /// Removes the value of `key`, and returns it, if it is there.
    pub fn remove(&mut self, key: MappingKey) -> Option<T> {
        let (removed, _) = remove(&mut self.root, &key)?;
        self.len -= 1;
        Some(removed.value)
    }

    /// Calls `f` with each value that maps part of the `len` bytes from `offset`, in order.
    pub fn for_each_in_range<'a>(&'a self, offset: u64, len: u64, mut f: impl FnMut(&'a T)) {
        visit(&self.root, offset, offset + len, &mut f, &mut || ());
    }
}

fn height<T>(link: &Link<T>) -> u8 {
    link.as_ref().map_or(0, |node| node.height)
}

fn max_end<T>(link: &Link<T>) -> u64 {
    link.as_ref().map_or(0, |node| node.max_end)
}

impl<T> Node<T> {
    /// Updates what it keeps of its subtree from its children's.
    fn update(&mut self) {
        self.height = 1 + height(&self.left).max(height(&self.right));
        self.max_end = self.end.max(max_end(&self.left)).max(max_end(&self.right));
    }

    /// How much taller its left subtree is than its right.
    fn balance(&self) -> i16 {
        i16::from(height(&self.left)) - i16::from(height(&self.right))
    }
}

/// Makes `node`'s left child the root of its subtree, `node` its right child.
fn rotate_right<T>(mut node: Box<Node<T>>) -> Box<Node<T>> {
    let mut left = node.left.take().expect("a left child to rotate up");
    node.left = left.right.take();
    node.update();
    left.right = Some(node);
    left.update();
    left
}

/// Makes `node`'s right child the root of its subtree, `node` its left child.
fn rotate_left<T>(mut node: Box<Node<T>>) -> Box<Node<T>> {
    let mut right = node.right.take().expect("a right child to rotate up");
    node.right = right.left.take();
    node.update();
    right.left = Some(node);
    right.update();
    right
}

/// Updates `node`, whose children are balanced and differ in height by at most two, and balances
/// its subtree, returning its new root.
fn rebalance<T>(mut node: Box<Node<T>>) -> Box<Node<T>> {
    node.update();
    let balance = node.balance();
    if balance > 1 {
        let left = node.left.take().expect("a taller left subtree");
        node.left = Some(if left.balance() < 0 { rotate_left(left) } else { left });
        return rotate_right(node);
    }
    if balance < -1 {
        let right = node.right.take().expect("a taller right subtree");
        node.right = Some(if right.balance() > 0 { rotate_right(right) } else { right });
        return rotate_left(node);
    }
    node
}

/// Rebalances the subtree at `link`, one of whose root's subtrees changed, and returns whether
/// what its root keeps of it changed too: its height or greatest end. Its ancestors need not be
/// updated otherwise, so that a change deep in the tree touches only the nodes around it.
fn rebalance_at<T>(link: &mut Link<T>) -> bool {
    let node = link.take().expect("a subtree that changed");
    let kept = (node.height, node.max_end);
    let node = rebalance(node);
    let changed = (node.height, node.max_end) != kept;
    *link = Some(node);
    changed
}

/// Inserts `new` into the subtree at `link`, unless its key is there, and returns whether it did
/// and, if so, whether what the subtree's root keeps of it changed.
fn insert<T>(link: &mut Link<T>, new: Box<Node<T>>) -> Option<bool> {
    let Some(node) = link else {
        *link = Some(new);
        return Some(true);
    };
    let child = match new.key.cmp(&node.key) {
        Ordering::Less => &mut node.left,
        Ordering::Greater => &mut node.right,
        Ordering::Equal => return None,
    };
    Some(insert(child, new)? && rebalance_at(link))
}

/// Removes the node of `key` from the subtree at `link`, if it is there, and returns it and
/// whether what the subtree's root keeps of it changed.
fn remove<T>(link: &mut Link<T>, key: &MappingKey) -> Option<(Box<Node<T>>, bool)> {
    let node = link.as_mut()?;
    let child = match key.cmp(&node.key) {
        Ordering::Less => &mut node.left,
        Ordering::Greater => &mut node.right,
        Ordering::Equal => {
            let mut removed = link.take().expect("the node found");
            *link = match (removed.left.take(), removed.right.take()) {
                (None, None) => None,
                (Some(child), None) | (None, Some(child)) => Some(child),
                // Its successor, the least of its right subtree, takes its place.
                (Some(left), Some(right)) => {
                    let mut right = Some(right);
                    let (mut successor, _) = remove_least(&mut right);
                    successor.left = Some(left);
                    successor.right = right;
                    Some(rebalance(successor))
                }
            };
            return Some((removed, true));
        }
    };
    let (removed, child_changed) = remove(child, key)?;
    Some((removed, child_changed && rebalance_at(link)))
}

/// Removes the least node of the subtree at `link`, which must have one, and returns it and
/// whether what the subtree's root keeps of it changed.
fn remove_least<T>(link: &mut Link<T>) -> (Box<Node<T>>, bool) {
    let node = link.as_mut().expect("a subtree to remove the least node of");
    if node.left.is_some() {
        let (least, child_changed) = remove_least(&mut node.left);
        return (least, child_changed && rebalance_at(link));
    }
    let mut least = link.take().expect("the least node");
    *link = least.right.take();
    (least, true)
}

/// Calls `f` with each value of the subtree at `link` that maps part of [offset, end), in order,
/// and `look` as it looks at each of its nodes: what a walk costs, which its tests count.
fn visit<'a, T>(
    link: &'a Link<T>,
    offset: u64,
    end: u64,
    f: &mut impl FnMut(&'a T),
    look: &mut impl FnMut(),
) {
    let Some(node) = link else { return };
    look();
    // Every mapping below ends at or before the range starts.
    if node.max_end <= offset {
        return;
    }
    visit(&node.left, offset, end, f, look);
    // It, and every mapping to its right, starts at or after the range ends.
    if node.key.object_offset >= end {
        return;
    }
    if node.end > offset {
        f(&node.value);
    }
    visit(&node.right, offset, end, f, look);
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: u64 = 4096;

    impl<T> MappingTree<T> {
        /// The values `for_each_in_range` finds.
        fn in_range(&self, offset: u64, len: u64) -> Vec<&T> {
            let mut found = Vec::new();
            self.for_each_in_range(offset, len, |value| found.push(value));
            found
        }

        /// How many nodes `for_each_in_range`'s walk looks at.
        fn looked_at(&self, offset: u64, len: u64) -> usize {
            let mut looked_at = 0;
            visit(&self.root, offset, offset + len, &mut |_| (), &mut || looked_at += 1);
            looked_at
        }

        fn height(&self) -> usize {
            usize::from(height(&self.root))
        }

        /// Panics unless the tree is in order and balanced, and each node keeps its subtree's
        /// height and greatest end; returns its keys and ends, in order.
        fn check(&self) -> Vec<(MappingKey, u64)> {
            fn check<T>(link: &Link<T>, entries: &mut Vec<(MappingKey, u64)>) -> (u8, u64) {
                let Some(node) = link else { return (0, 0) };
                let (left_height, left_max_end) = check(&node.left, entries);
                if let Some((last, _)) = entries.last() {
                    assert!(*last < node.key, "{last:?} before {:?}", node.key);
                }
                entries.push((node.key, node.end));
                let (right_height, right_max_end) = check(&node.right, entries);
                assert!(left_height.abs_diff(right_height) <= 1, "unbalanced at {:?}", node.key);
                assert_eq!(node.height, 1 + left_height.max(right_height));
                assert_eq!(node.max_end, node.end.max(left_max_end).max(right_max_end));
                (node.height, node.max_end)
            }
            let mut entries = Vec::new();
            check(&self.root, &mut entries);
            assert_eq!(entries.len(), self.len);
            entries
        }
    }

    fn key(page: u64, id: usize) -> MappingKey {
        MappingKey { object_offset: page * PAGE, id }
    }

    /// A deterministic sequence of numbers in [0, bound), for the shapes the tests make.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, bound: u64) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) % bound
        }
    }

    /// A VMO's mappings in a list in order of their keys, which the tree must agree with: each
    /// one's key, end and value.
    #[derive(Default)]
    struct Model(Vec<(MappingKey, u64, u64)>);

    impl Model {
        fn insert(&mut self, key: MappingKey, len: u64, value: u64) -> bool {
            if self.0.iter().any(|(kept, ..)| *kept == key) {
                return false;
            }
            let at = self.0.partition_point(|(kept, ..)| *kept < key);
            self.0.insert(at, (key, key.object_offset + len, value));
            true
        }

        fn remove(&mut self, key: MappingKey) -> Option<u64> {
            let at = self.0.iter().position(|(kept, ..)| *kept == key)?;
            Some(self.0.remove(at).2)
        }

        /// The values of the mappings of part of the `len` bytes from `offset`, in order.
        fn in_range(&self, offset: u64, len: u64) -> impl Iterator<Item = &u64> {
            (self.0.iter())
                .filter(move |(key, end, _)| key.object_offset < offset + len && *end > offset)
                .map(|(.., value)| value)
        }
    }

    /// Panics unless `tree`, which must be in order and balanced, keeps the mappings `model` does,
    /// and finds the values it finds in each of `ranges`, of an offset and a length.
    fn assert_agree(
        tree: &MappingTree<u64>,
        model: &Model,
        ranges: impl IntoIterator<Item = (u64, u64)>,
    ) {
        let kept: Vec<(MappingKey, u64)> =
            model.0.iter().map(|&(key, end, _)| (key, end)).collect();
        assert_eq!(tree.check(), kept);
        assert_eq!(tree.is_empty(), kept.is_empty());
        for (offset, len) in ranges {
            let mut expected = model.in_range(offset, len);
            tree.for_each_in_range(offset, len, |value| {
                assert_eq!(Some(value), expected.next(), "{len:#x} bytes from {offset:#x}");
            });
            assert_eq!(expected.next(), None, "{len:#x} bytes from {offset:#x}");
        }
    }

    /// Mappings added and removed at random, of one to many pages from anywhere among 4 to 1024,
    /// many of the same offset, with keys added again and removed that are not there: the tree
    /// keeps what a list of them in order keeps, returns what it returns, finds in ranges what a
    /// scan of the list finds, in order, and stays in order and balanced throughout, as it grows
    /// and as it is emptied, three times over.
    #[test]
    fn a_ranges_mappings_are_those_a_scan_finds() {
        for seed in 0..16 {
            let mut lcg = Lcg(92_50 + seed);
            // The pages the mappings map from.
            let span = [4, 16, 64, 1024][seed as usize % 4];
            let (mut tree, mut model) = (MappingTree::new(), Model::default());
            let mut value = 0;
            for _ in 0..3 {
                // Adding two times in three while it grows, then removing four times in five until
                // it is empty.
                let mut step = 0;
                while step < 600 || !model.0.is_empty() {
                    let adding = if step < 600 { lcg.next(3) != 0 } else { lcg.next(5) == 0 };
                    // One of those it keeps, mostly to remove, or another, mostly to add, which may
                    // be of an offset of those it keeps.
                    let kept = !model.0.is_empty() && lcg.next(8) < if adding { 1 } else { 7 };
                    let key = if kept {
                        model.0[lcg.next(model.0.len() as u64) as usize].0
                    } else {
                        key(lcg.next(span), lcg.next(1 << 16) as usize)
                    };
                    if adding {
                        let pages = 1 + if lcg.next(8) == 0 { lcg.next(span) } else { lcg.next(3) };
                        value += 1;
                        let added = model.insert(key, pages * PAGE, value);
                        assert_eq!(tree.insert(key, pages * PAGE, value), added, "{seed}: {step}");
                    } else {
                        assert_eq!(tree.remove(key), model.remove(key), "{seed}: {step}");
                    }
                    let mut range =
                        || (lcg.next(span + 4) * PAGE, (1 + lcg.next(span.min(16))) * PAGE);
                    assert_agree(&tree, &model, [range(), range(), range()]);
                    step += 1;
                }
            }
        }
    }

    /// Every order of `0..count`.
    fn orders_of(count: usize) -> Vec<Vec<usize>> {
        let Some(last) = count.checked_sub(1) else { return vec![Vec::new()] };
        let shorter = orders_of(last);
        let mut orders = Vec::new();
        for order in &shorter {
            for at in 0..count {
                let mut order = order.clone();
                order.insert(at, last);
                orders.push(order);
            }
        }
        orders
    }

    /// Every order of adding up to five mappings, and every order of removing them then, which
    /// makes each of the tree's rotations, single and double, either way, of a node whose taller
    /// child is balanced too, and each kind of removal, of a leaf, of a node of one child, and of
    /// one of two, whose successor is its right child or below it, with a right child or none: the
    /// tree keeps what a list of them in order keeps after each, and finds what it finds in every
    /// range of the pages they map. Two of the mappings are of each offset, and the greatest end is
    /// the first's or the last's.
    #[test]
    fn every_order_of_adding_and_removing_a_few_mappings() {
        let ranges: Vec<(u64, u64)> = (0..8)
            .flat_map(|first| (first + 1..=8).map(move |end| (first * PAGE, (end - first) * PAGE)))
            .collect();
        // The pages of the `n`th of `count` mappings: more the later it is, or the earlier.
        let lens: [fn(u64, u64) -> u64; 2] = [|n, _| 1 + n, |n, count| count - n];
        for count in 1..=5 {
            let orders = orders_of(count as usize);
            for pages in lens {
                // The `n`th mapping's key, of the offset of one other, length and value.
                let mapping = |n: usize| {
                    let n = n as u64;
                    (key(n / 2, n as usize), pages(n, count) * PAGE, n)
                };
                for adding in &orders {
                    for removing in &orders {
                        let (mut tree, mut model) = (MappingTree::new(), Model::default());
                        for &n in adding {
                            let (key, len, value) = mapping(n);
                            assert!(model.insert(key, len, value));
                            assert!(tree.insert(key, len, value));
                            assert_agree(&tree, &model, ranges.iter().copied());
                        }
                        for &n in removing {
                            let (key, _, value) = mapping(n);
                            assert_eq!(model.remove(key), Some(value));
                            assert_eq!(tree.remove(key), Some(value));
                            assert_agree(&tree, &model, ranges.iter().copied());
                        }
                    }
                }
            }
        }
    }

    /// A page's walk among 100k mappings of a page each, one for each page, looks at the nodes on
    /// the path to the page, at most one beside each, and those on the path from it down to the
    /// page after it: at most three for each of the tree's 19 levels, and 18 to 31 nodes here, not
    /// at the mappings. Each mapping is found, alone, however it was added, by a range of it, and
    /// none by a range past them, whose walk looks at the root alone.
    #[test]
    fn a_pages_walk_looks_at_a_path_of_the_tree() {
        const MAPPINGS: u64 = 100_000;
        let mut tree = MappingTree::new();
        // Added in order, then in an order spread over them, by a stride coprime with their count.
        let spread = (0..MAPPINGS / 2).map(|n| MAPPINGS / 2 + n * 7919 % (MAPPINGS / 2));
        for page in (0..MAPPINGS / 2).chain(spread) {
            assert!(tree.insert(key(page, page as usize), PAGE, page));
        }
        tree.check();
        assert_eq!(tree.len(), MAPPINGS as usize);
        assert_eq!(tree.height(), 19);
        let pages = (0..MAPPINGS).step_by(997).chain([MAPPINGS - 1]);
        let looked_at: Vec<usize> = pages
            .map(|page| {
                assert_eq!(tree.in_range(page * PAGE, PAGE), [&page]);
                tree.looked_at(page * PAGE, PAGE)
            })
            .collect();
        let fewest_and_most = (looked_at.iter().min(), looked_at.iter().max());
        assert_eq!(fewest_and_most, (Some(&18), Some(&31)));
        assert_eq!(tree.in_range(MAPPINGS * PAGE, PAGE), Vec::<&u64>::new());
        assert_eq!(tree.looked_at(MAPPINGS * PAGE, PAGE), 1);
    }

    /// A mapping of all the pages, at the first of them, is found by a page's walk among 100k
    /// mappings of a page each, though it starts before it. The walk looks at the nodes it would
    /// without it, at most three for each level of the tree, and at those on the path to it and
    /// beside it, which it cannot skip, as it ends after the page: at most two more a level, of the
    /// tree's 17. It looks at 18 nodes for the second page, 51 for the 33333rd and 63 for the last.
    #[test]
    fn a_pages_walk_finds_a_mapping_that_starts_before_it() {
        const MAPPINGS: u64 = 100_000;
        let mut tree = MappingTree::new();
        assert!(tree.insert(key(0, usize::MAX), MAPPINGS * PAGE, u64::MAX));
        for page in 0..MAPPINGS {
            assert!(tree.insert(key(page, page as usize), PAGE, page));
        }
        tree.check();
        assert_eq!(tree.height(), 17);
        let pages = [1, MAPPINGS / 3, MAPPINGS - 1];
        let looked_at = pages.map(|page| {
            assert_eq!(tree.in_range(page * PAGE, PAGE), [&u64::MAX, &page]);
            tree.looked_at(page * PAGE, PAGE)
        });
        assert_eq!(looked_at, [18, 51, 63]);
    }
}
