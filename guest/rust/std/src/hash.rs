//! Hashing: `core`'s, and std's randomly keyed SipHash.

use crate::sys;
#[allow(deprecated)]
use core::hash::SipHasher;
pub use core::hash::*;

/// Builds `DefaultHasher`s with keys the kernel's generator picked.
#[derive(Clone, Debug)]
pub struct RandomState {
    k0: u64,
    k1: u64,
}

impl RandomState {
    pub fn new() -> Self {
        Self { k0: sys::random_u64(), k1: sys::random_u64() }
    }
}

impl Default for RandomState {
    fn default() -> Self {
        Self::new()
    }
}

impl BuildHasher for RandomState {
    type Hasher = DefaultHasher;
    fn build_hasher(&self) -> DefaultHasher {
        #[allow(deprecated)]
        DefaultHasher(SipHasher::new_with_keys(self.k0, self.k1))
    }
}

/// SipHash, as std's hash maps use it.
#[allow(deprecated)]
#[derive(Clone, Debug)]
pub struct DefaultHasher(SipHasher);

impl DefaultHasher {
    #[allow(deprecated)]
    pub fn new() -> Self {
        Self(SipHasher::new_with_keys(0, 0))
    }
}

impl Default for DefaultHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Hasher for DefaultHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes)
    }

    fn finish(&self) -> u64 {
        self.0.finish()
    }
}
