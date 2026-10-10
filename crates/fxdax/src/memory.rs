//! Memory fxdax copies into and out of, which something else may change while it does, such as a
//! guest's memory.
//!
//! A guest may write to its buffers while the device reads them, so a Rust reference to them
//! would be unsound. fxdax refers to such memory only by address and length, as the kernel's user
//! copies do, and only ever copies into or out of it, as `vm-memory`'s volatile slices do.

use std::marker::PhantomData;
use zircon::user_copy::HostBuffer;

/// `len` bytes of memory that fxdax copies out of, such as a request's buffer in guest memory.
#[derive(Clone, Copy, Debug)]
pub struct VolatileSlice<'a> {
    addr: usize,
    len: usize,
    memory: PhantomData<&'a [u8]>,
}

/// `len` bytes of memory that fxdax copies into, such as a reply's buffer in guest memory.
#[derive(Debug)]
pub struct VolatileSliceMut<'a> {
    addr: usize,
    len: usize,
    memory: PhantomData<&'a mut [u8]>,
}

// SAFETY: The slices are addresses and lengths, through which memory is only copied, from any
// thread, as their creators guarantee it may be for their lifetimes. A `VolatileSliceMut` is
// copied into only through `&mut`, as it may be memory Rust owns (`From<&mut [u8]>`), so it is not
// `Sync`: shared, it could be written from two threads at once.
unsafe impl Send for VolatileSlice<'_> {}
unsafe impl Sync for VolatileSlice<'_> {}
unsafe impl Send for VolatileSliceMut<'_> {}

impl<'a> VolatileSlice<'a> {
    /// The `len` bytes at `ptr`.
    ///
    /// # Safety
    ///
    /// The memory must be mapped, and readable, for `'a`. Others may write to it concurrently.
    pub unsafe fn from_raw_parts(ptr: *const u8, len: usize) -> Self {
        Self { addr: ptr as usize, len, memory: PhantomData }
    }

    /// The length in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the slice is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The `len` bytes at `offset` into the slice, if they are within it.
    pub fn subslice(&self, offset: usize, len: usize) -> Option<Self> {
        let end = offset.checked_add(len)?;
        (end <= self.len).then(|| Self { addr: self.addr + offset, len, memory: PhantomData })
    }

    /// Copies the slice's first bytes into `dst`, as many as fit, and returns how many.
    pub fn copy_to(&self, dst: &mut [u8]) -> usize {
        let len = self.len.min(dst.len());
        // SAFETY: The slice's memory is readable for `len` bytes, and `dst` is another's.
        unsafe { std::ptr::copy_nonoverlapping(self.addr as *const u8, dst.as_mut_ptr(), len) };
        len
    }

    pub(crate) fn host_buffer(&self) -> HostBuffer {
        HostBuffer { addr: self.addr, len: self.len }
    }
}

impl<'a> From<&'a [u8]> for VolatileSlice<'a> {
    fn from(slice: &'a [u8]) -> Self {
        // SAFETY: The slice is readable for `'a`.
        unsafe { Self::from_raw_parts(slice.as_ptr(), slice.len()) }
    }
}

impl<'a> VolatileSliceMut<'a> {
    /// The `len` bytes at `ptr`.
    ///
    /// # Safety
    ///
    /// The memory must be mapped, and readable and writable, for `'a`. Others may read and write
    /// it concurrently.
    pub unsafe fn from_raw_parts(ptr: *mut u8, len: usize) -> Self {
        Self { addr: ptr as usize, len, memory: PhantomData }
    }

    /// The length in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the slice is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The `len` bytes at `offset` into the slice, if they are within it.
    pub fn subslice(&mut self, offset: usize, len: usize) -> Option<VolatileSliceMut<'_>> {
        let end = offset.checked_add(len)?;
        (end <= self.len).then(|| VolatileSliceMut {
            addr: self.addr + offset,
            len,
            memory: PhantomData,
        })
    }

    /// Splits the slice in two at `mid`, which must be within it.
    pub fn split_at(self, mid: usize) -> (Self, Self) {
        assert!(mid <= self.len, "{mid} is past the end of the slice");
        let tail = Self { addr: self.addr + mid, len: self.len - mid, memory: PhantomData };
        (Self { len: mid, ..self }, tail)
    }

    /// Copies `src` into the slice's first bytes, as many as fit, and returns how many.
    pub fn copy_from(&mut self, src: &[u8]) -> usize {
        let len = self.len.min(src.len());
        // SAFETY: The slice's memory is writable for `len` bytes, and `src` is another's.
        unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), self.addr as *mut u8, len) };
        len
    }

    /// The slice, to copy out of.
    pub fn as_volatile_slice(&self) -> VolatileSlice<'_> {
        VolatileSlice { addr: self.addr, len: self.len, memory: PhantomData }
    }

    pub(crate) fn host_buffer(&self) -> HostBuffer {
        HostBuffer { addr: self.addr, len: self.len }
    }

    /// The address of the byte at `offset`, which must be within the slice.
    pub(crate) fn addr_at(&self, offset: usize) -> usize {
        assert!(offset <= self.len, "{offset} is past the end of the slice");
        self.addr + offset
    }
}

impl<'a> From<&'a mut [u8]> for VolatileSliceMut<'a> {
    fn from(slice: &'a mut [u8]) -> Self {
        // SAFETY: The slice is readable and writable for `'a`.
        unsafe { Self::from_raw_parts(slice.as_mut_ptr(), slice.len()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_and_subslices() {
        let source = *b"hello, world";
        let slice = VolatileSlice::from(&source[..]);
        assert_eq!(slice.len(), 12);
        let mut out = [0u8; 5];
        assert_eq!(slice.subslice(7, 5).unwrap().copy_to(&mut out), 5);
        assert_eq!(&out, b"world");
        assert!(slice.subslice(8, 5).is_none());
        assert!(slice.subslice(usize::MAX, 2).is_none());

        let mut target = [0u8; 8];
        let (mut head, mut tail) = VolatileSliceMut::from(&mut target[..]).split_at(3);
        assert_eq!(head.copy_from(b"abcdef"), 3);
        assert_eq!(tail.copy_from(b"xy"), 2);
        assert_eq!(&target, b"abcxy\0\0\0");
    }
}
