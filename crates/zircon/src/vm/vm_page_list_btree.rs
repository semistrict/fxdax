// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The B-tree of page list nodes behind `VmPageList`.
//!
//! Zircon keeps `VmPageListNode`s in a C++ B-tree keyed by node offset, and
//! the Rust `VmPageList` reaches it through `cpp_vm_page_list_btree_*`
//! bindings (`vm_page_list_ffi.cc`). This provides the same functions over a
//! `BTreeMap`, so `vm_page_list.rs` is unchanged apart from its imports.
//!
//! Nodes are boxed so that pointers to them stay valid while other nodes are
//! inserted and removed, as they do in the C++ tree. A cursor records the key
//! it is at rather than a position in the tree, which is equivalent because
//! keys never change.

#![allow(clippy::missing_safety_doc)]

use super::vm_page_list::VmPageListNode;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::ops::Bound;

pub type VmPageListBtree = BTreeMap<u64, Box<VmPageListNode>>;

pub struct VmPageListBtreeCursor {
    tree: *mut VmPageListBtree,
    /// The key of the node the cursor is at, or `None` if it is past the end.
    key: Option<u64>,
}

pub type VmPageListBtreeConstCursor = VmPageListBtreeCursor;

pub struct VmPageListBtreeNodeEntry {
    pub offset: u64,
    pub node: *mut c_void,
}

pub type VmPageListBtreeConstNodeEntry = VmPageListBtreeNodeEntry;

const INVALID: VmPageListBtreeNodeEntry =
    VmPageListBtreeNodeEntry { offset: 0, node: std::ptr::null_mut() };

unsafe fn entry(tree: *mut VmPageListBtree, key: Option<u64>) -> VmPageListBtreeNodeEntry {
    let Some(key) = key else { return INVALID };
    // SAFETY: The caller passes a valid tree.
    match unsafe { (*tree).get_mut(&key) } {
        Some(node) => VmPageListBtreeNodeEntry { offset: key, node: (&raw mut **node).cast() },
        None => INVALID,
    }
}

unsafe fn key_after(tree: *mut VmPageListBtree, key: u64) -> Option<u64> {
    // SAFETY: The caller passes a valid tree.
    unsafe { (*tree).range((Bound::Excluded(key), Bound::Unbounded)).next().map(|(k, _)| *k) }
}

pub unsafe fn cpp_vm_page_list_btree_init(tree: *mut VmPageListBtree) {
    // SAFETY: `tree` points to uninitialized storage for a tree.
    unsafe { tree.write(BTreeMap::new()) }
}

pub unsafe fn cpp_vm_page_list_btree_destroy(tree: *mut VmPageListBtree) {
    // SAFETY: `tree` was initialized and is not used again.
    unsafe { std::ptr::drop_in_place(tree) }
}

pub unsafe fn cpp_vm_page_list_btree_is_empty(tree: *const VmPageListBtree) -> bool {
    // SAFETY: `tree` is valid.
    unsafe { (*tree).is_empty() }
}

pub unsafe fn cpp_vm_page_list_btree_clear(tree: *mut VmPageListBtree) {
    // SAFETY: `tree` is valid.
    unsafe { (*tree).clear() }
}

pub unsafe fn cpp_vm_page_list_btree_find(
    tree: *mut VmPageListBtree,
    node_offset: u64,
    out_cursor: *mut VmPageListBtreeCursor,
) -> *mut c_void {
    // SAFETY: `tree` is valid.
    let found = unsafe { entry(tree, Some(node_offset)) };
    if !found.node.is_null() && !out_cursor.is_null() {
        // SAFETY: `out_cursor` points to storage for a cursor.
        unsafe { out_cursor.write(VmPageListBtreeCursor { tree, key: Some(node_offset) }) };
    }
    found.node
}

pub unsafe fn cpp_vm_page_list_btree_find_const(
    tree: *const VmPageListBtree,
    node_offset: u64,
) -> *const c_void {
    // SAFETY: `tree` is valid.
    unsafe { cpp_vm_page_list_btree_find(tree.cast_mut(), node_offset, std::ptr::null_mut()) }
}

pub unsafe fn cpp_vm_page_list_btree_lower_bound(
    tree: *mut VmPageListBtree,
    node_offset: u64,
    out_cursor: *mut VmPageListBtreeCursor,
) -> VmPageListBtreeNodeEntry {
    // SAFETY: `tree` is valid.
    let key = unsafe { (*tree).range(node_offset..).next().map(|(k, _)| *k) };
    if !out_cursor.is_null() {
        // SAFETY: `out_cursor` points to storage for a cursor.
        unsafe { out_cursor.write(VmPageListBtreeCursor { tree, key }) };
    }
    // SAFETY: `tree` is valid.
    unsafe { entry(tree, key) }
}

pub unsafe fn cpp_vm_page_list_btree_cursor_get(
    cursor: *const VmPageListBtreeCursor,
) -> VmPageListBtreeNodeEntry {
    // SAFETY: `cursor` is initialized.
    let cursor = unsafe { &*cursor };
    if cursor.tree.is_null() {
        return INVALID;
    }
    // SAFETY: A cursor's tree outlives it.
    unsafe { entry(cursor.tree, cursor.key) }
}

pub unsafe fn cpp_vm_page_list_btree_insert(
    tree: *mut VmPageListBtree,
    node_offset: u64,
    cursor: *mut VmPageListBtreeCursor,
) -> *mut c_void {
    let mut node = Box::new(VmPageListNode::new());
    let raw: *mut c_void = (&raw mut *node).cast();
    // SAFETY: `tree` is valid.
    let previous = unsafe { (*tree).insert(node_offset, node) };
    debug_assert!(previous.is_none(), "node at {node_offset:#x} already exists");
    if !cursor.is_null() {
        // SAFETY: `cursor` points to storage for a cursor.
        unsafe { cursor.write(VmPageListBtreeCursor { tree, key: Some(node_offset) }) };
    }
    raw
}

pub unsafe fn cpp_vm_page_list_btree_erase_at(
    tree: *mut VmPageListBtree,
    cursor: *mut VmPageListBtreeCursor,
) {
    // SAFETY: `cursor` is initialized and at a valid node of `tree`.
    unsafe {
        let key = (*cursor).key.expect("erase at an invalid cursor");
        (*tree).remove(&key);
        (*cursor).key = key_after(tree, key);
    }
}

pub unsafe fn cpp_vm_page_list_btree_cursor_default_init(cursor: *mut VmPageListBtreeCursor) {
    // SAFETY: `cursor` points to storage for a cursor.
    unsafe { cursor.write(VmPageListBtreeCursor { tree: std::ptr::null_mut(), key: None }) };
}

pub unsafe fn cpp_vm_page_list_btree_cursor_init(
    cursor: *mut VmPageListBtreeCursor,
    tree: *mut VmPageListBtree,
) {
    // SAFETY: `tree` is valid and `cursor` points to storage for a cursor.
    unsafe {
        let key = (*tree).keys().next().copied();
        cursor.write(VmPageListBtreeCursor { tree, key });
    }
}

pub unsafe fn cpp_vm_page_list_btree_cursor_next(
    cursor: *mut VmPageListBtreeCursor,
) -> VmPageListBtreeNodeEntry {
    // SAFETY: `cursor` is initialized.
    let current = unsafe { cpp_vm_page_list_btree_cursor_get(cursor) };
    if !current.node.is_null() {
        // SAFETY: The cursor is at a node of its tree.
        unsafe { (*cursor).key = key_after((*cursor).tree, current.offset) };
    }
    current
}

pub unsafe fn cpp_vm_page_list_btree_cursor_prev(
    cursor: *mut VmPageListBtreeCursor,
) -> VmPageListBtreeNodeEntry {
    // SAFETY: `cursor` is initialized with a tree.
    unsafe {
        let tree = (*cursor).tree;
        let key = match (*cursor).key {
            Some(key) => (*tree).range(..key).next_back().map(|(k, _)| *k),
            None => (*tree).keys().next_back().copied(),
        };
        (*cursor).key = key;
        entry(tree, key)
    }
}

pub unsafe fn cpp_vm_page_list_btree_const_cursor_init(
    cursor: *mut VmPageListBtreeConstCursor,
    tree: *const VmPageListBtree,
) {
    // SAFETY: Forwarded.
    unsafe { cpp_vm_page_list_btree_cursor_init(cursor, tree.cast_mut()) }
}

pub unsafe fn cpp_vm_page_list_btree_const_cursor_next(
    cursor: *mut VmPageListBtreeConstCursor,
) -> VmPageListBtreeConstNodeEntry {
    // SAFETY: Forwarded.
    unsafe { cpp_vm_page_list_btree_cursor_next(cursor) }
}
