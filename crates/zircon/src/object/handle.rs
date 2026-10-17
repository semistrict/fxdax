// Copyright 2016 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Handles and the handle table.
//!
//! Ported from `zircon/kernel/object/handle.cc`,
//! `zircon/kernel/object/handle_table.cc` and `zircon/kernel/object/handle.rs`.
//!
//! A handle pairs a dispatcher with the rights its holder has to it. Closing the last handle to
//! an object calls its `on_zero_handles`.
//!
//! Deviations:
//! - The kernel runs a single process, so there is one handle table.
//! - Handle values encode a slot and a generation, with the low bits set as Zircon requires,
//!   rather than being derived from the handle's address and a per-table random value.
//! - Job policy is not enforced.

use super::dispatcher::{Dispatcher, downcast};
use crate::ensure;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use zx_status::Status;
use zx_types::{ZX_HANDLE_FIXED_BITS_MASK, ZX_HANDLE_INVALID, zx_handle_t, zx_rights_t};

/// A reference to a dispatcher with a set of rights.
pub struct Handle {
    dispatcher: Arc<dyn Dispatcher>,
    rights: zx_rights_t,
    /// Identifies this handle to observers registered through it.
    id: u64,
}

/// Owns a handle; dropping it closes the handle.
pub type HandleOwner = Box<Handle>;

impl Handle {
    /// Makes a handle to `dispatcher`, counting it against the dispatcher's handles.
    pub fn make(dispatcher: Arc<dyn Dispatcher>, rights: zx_rights_t) -> HandleOwner {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        dispatcher.base().increment_handle_count();
        Box::new(Handle { dispatcher, rights, id: NEXT_ID.fetch_add(1, Ordering::Relaxed) })
    }

    /// Makes a handle to the same object with `rights`.
    pub fn dup(&self, rights: zx_rights_t) -> HandleOwner {
        Self::make(self.dispatcher.clone(), rights)
    }

    pub fn dispatcher(&self) -> &Arc<dyn Dispatcher> {
        &self.dispatcher
    }

    pub fn rights(&self) -> zx_rights_t {
        self.rights
    }

    pub fn has_rights(&self, desired: zx_rights_t) -> bool {
        (self.rights & desired) == desired
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if self.dispatcher.is_waitable() {
            self.dispatcher.base().cancel(self.id);
        }
        if self.dispatcher.base().decrement_handle_count() {
            self.dispatcher.on_zero_handles();
        }
    }
}

/// A newly created dispatcher that has not yet been given a handle. If it is dropped without
/// becoming a handle, the dispatcher is told it has no handles.
pub struct KernelHandle<T: Dispatcher> {
    dispatcher: Option<Arc<T>>,
}

impl<T: Dispatcher> KernelHandle<T> {
    pub fn new(dispatcher: Arc<T>) -> Self {
        Self { dispatcher: Some(dispatcher) }
    }

    pub fn dispatcher(&self) -> &Arc<T> {
        self.dispatcher.as_ref().unwrap()
    }

    /// Releases the dispatcher without it being told it has no handles, for the kernel to keep.
    pub fn release(mut self) -> Arc<T> {
        self.dispatcher.take().unwrap()
    }

    /// Adds a handle with `rights` to the handle table, returning its value.
    pub fn make_and_add_handle(mut self, rights: zx_rights_t) -> Result<zx_handle_t, Status> {
        let dispatcher: Arc<dyn Dispatcher> = self.dispatcher.take().unwrap();
        handle_table().add_handle(Handle::make(dispatcher, rights))
    }
}

impl<T: Dispatcher> Drop for KernelHandle<T> {
    fn drop(&mut self) {
        if let Some(dispatcher) = self.dispatcher.take() {
            dispatcher.on_zero_handles();
        }
    }
}

struct Slot {
    generation: u32,
    handle: Option<HandleOwner>,
}

/// The process's handles.
pub struct HandleTable {
    slots: Mutex<(Vec<Slot>, Vec<u32>)>,
}

const INDEX_BITS: u32 = 18;
const GENERATION_BITS: u32 = 30 - INDEX_BITS;

impl HandleTable {
    fn value(index: u32, generation: u32) -> zx_handle_t {
        (((generation << INDEX_BITS) | index) << 2) | ZX_HANDLE_FIXED_BITS_MASK
    }

    fn slot_of(value: zx_handle_t) -> Option<(usize, u32)> {
        if value == ZX_HANDLE_INVALID
            || (value & ZX_HANDLE_FIXED_BITS_MASK) != ZX_HANDLE_FIXED_BITS_MASK
        {
            return None;
        }
        let bits = value >> 2;
        Some(((bits & ((1 << INDEX_BITS) - 1)) as usize, bits >> INDEX_BITS))
    }

    pub fn add_handle(&self, handle: HandleOwner) -> Result<zx_handle_t, Status> {
        let mut guard = self.slots.lock();
        let (slots, free) = &mut *guard;
        let index = match free.pop() {
            Some(index) => index,
            None => {
                ensure!(slots.len() < 1 << INDEX_BITS, Status::NO_RESOURCES);
                slots.push(Slot { generation: 0, handle: None });
                (slots.len() - 1) as u32
            }
        };
        let slot = &mut slots[index as usize];
        slot.handle = Some(handle);
        Ok(Self::value(index, slot.generation))
    }

    /// Removes and returns the handle with `value`.
    pub fn remove_handle(&self, value: zx_handle_t) -> Option<HandleOwner> {
        let (index, generation) = Self::slot_of(value)?;
        let mut guard = self.slots.lock();
        let (slots, free) = &mut *guard;
        let slot = slots.get_mut(index)?;
        if slot.generation != generation || slot.handle.is_none() {
            return None;
        }
        slot.generation = (slot.generation + 1) & ((1 << GENERATION_BITS) - 1);
        free.push(index as u32);
        slot.handle.take()
    }

    /// Duplicates the handle with `value`, removing it if `is_replace`, as zx_handle_duplicate and
    /// zx_handle_replace do.
    pub fn dup_replace(
        &self,
        is_replace: bool,
        value: zx_handle_t,
        rights: zx_rights_t,
    ) -> Result<zx_handle_t, Status> {
        let (index, generation) = Self::slot_of(value).ok_or(Status::BAD_HANDLE)?;
        let mut guard = self.slots.lock();
        let (slots, free) = &mut *guard;
        let source = match slots.get(index) {
            Some(Slot { generation: g, handle: Some(handle) }) if *g == generation => handle,
            _ => return Err(Status::BAD_HANDLE),
        };
        ensure!(
            is_replace || source.has_rights(zx_types::ZX_RIGHT_DUPLICATE),
            Status::ACCESS_DENIED
        );
        let rights = if rights == zx_types::ZX_RIGHT_SAME_RIGHTS {
            source.rights()
        } else if (source.rights() & rights) != rights {
            if is_replace {
                let removed = Self::remove_locked(slots, free, index);
                drop(guard);
                drop(removed);
            }
            return Err(Status::INVALID_ARGS);
        } else {
            rights
        };
        let handle = source.dup(rights);
        let removed = if is_replace { Self::remove_locked(slots, free, index) } else { None };
        drop(guard);
        drop(removed);
        self.add_handle(handle)
    }

    fn remove_locked(slots: &mut [Slot], free: &mut Vec<u32>, index: usize) -> Option<HandleOwner> {
        let slot = &mut slots[index];
        slot.generation = (slot.generation + 1) & ((1 << GENERATION_BITS) - 1);
        free.push(index as u32);
        slot.handle.take()
    }

    /// Calls `f` with the handle with `value`, holding the table's lock.
    pub fn with_handle<R>(
        &self,
        value: zx_handle_t,
        f: impl FnOnce(&Handle) -> R,
    ) -> Result<R, Status> {
        let (index, generation) = Self::slot_of(value).ok_or(Status::BAD_HANDLE)?;
        let guard = self.slots.lock();
        let slot = guard.0.get(index).ok_or(Status::BAD_HANDLE)?;
        match &slot.handle {
            Some(handle) if slot.generation == generation => Ok(f(handle)),
            _ => Err(Status::BAD_HANDLE),
        }
    }

    /// Returns the dispatcher of the handle with `value` and the handle's rights.
    pub fn get_dispatcher_and_rights(
        &self,
        value: zx_handle_t,
    ) -> Result<(Arc<dyn Dispatcher>, zx_rights_t), Status> {
        self.with_handle(value, |handle| (handle.dispatcher.clone(), handle.rights))
    }

    /// Returns the dispatcher of type `T` of the handle with `value`, and the handle's rights.
    pub fn get_dispatcher_and_rights_as<T: Dispatcher>(
        &self,
        value: zx_handle_t,
    ) -> Result<(Arc<T>, zx_rights_t), Status> {
        let (dispatcher, rights) = self.get_dispatcher_and_rights(value)?;
        let dispatcher = downcast::<T>(dispatcher).ok_or(Status::WRONG_TYPE)?;
        Ok((dispatcher, rights))
    }

    /// Returns the dispatcher of type `T` of the handle with `value`, which must have `rights`.
    pub fn get_dispatcher_with_rights<T: Dispatcher>(
        &self,
        value: zx_handle_t,
        rights: zx_rights_t,
    ) -> Result<Arc<T>, Status> {
        let (dispatcher, actual) = self.get_dispatcher_and_rights_as::<T>(value)?;
        ensure!((actual & rights) == rights, Status::ACCESS_DENIED);
        Ok(dispatcher)
    }

    /// Returns the dispatcher of type `T` of the handle with `value`, with no rights check.
    pub fn get_dispatcher<T: Dispatcher>(&self, value: zx_handle_t) -> Result<Arc<T>, Status> {
        Ok(self.get_dispatcher_and_rights_as::<T>(value)?.0)
    }
}

/// Returns the process's handle table.
pub fn handle_table() -> &'static HandleTable {
    static TABLE: OnceLock<HandleTable> = OnceLock::new();
    TABLE.get_or_init(|| HandleTable { slots: Mutex::new((Vec::new(), Vec::new())) })
}
