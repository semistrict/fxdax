//! Collections: `alloc`'s, and std's hash maps and sets over hashbrown, which
//! scripts/import-guest-std-collections.py imports from std's sources (`hash`).

pub use alloc_crate::collections::*;
pub use hash_map::HashMap;
pub use hash_set::HashSet;

mod hash {
    pub mod map;
    pub mod set;
}

pub mod hash_map {
    //! A hash map implemented with quadratic probing and SIMD lookup.
    pub use super::hash::map::*;
    pub use crate::hash::{DefaultHasher, RandomState};
}

pub mod hash_set {
    //! A hash set implemented as a `HashMap` where the value is `()`.
    pub use super::hash::set::*;
}
