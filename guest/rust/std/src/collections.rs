//! Collections: `alloc`'s, and hash maps and sets, which are hashbrown's with std's hasher.

pub use alloc_crate::collections::*;
pub use hash_map::HashMap;
pub use hash_set::HashSet;

/// Wraps a hashbrown collection as std's: its methods, through `Deref`, std's constructors, and
/// the traits std's implements.
macro_rules! std_collection {
    ($name:ident<$($param:ident),*>, $inner:ty, $item:ty, $iter:ty) => {
        pub struct $name<$($param,)* S = crate::hash::RandomState>(pub(crate) $inner);

        impl<$($param),*> $name<$($param),*, crate::hash::RandomState> {
            pub fn new() -> Self {
                Self::with_hasher(Default::default())
            }

            pub fn with_capacity(capacity: usize) -> Self {
                Self::with_capacity_and_hasher(capacity, Default::default())
            }
        }

        impl<$($param,)* S> $name<$($param,)* S> {
            pub const fn with_hasher(hasher: S) -> Self {
                Self(<$inner>::with_hasher(hasher))
            }

            pub fn with_capacity_and_hasher(capacity: usize, hasher: S) -> Self {
                Self(<$inner>::with_capacity_and_hasher(capacity, hasher))
            }
        }

        impl<$($param,)* S> core::ops::Deref for $name<$($param,)* S> {
            type Target = $inner;
            fn deref(&self) -> &$inner {
                &self.0
            }
        }

        impl<$($param,)* S> core::ops::DerefMut for $name<$($param,)* S> {
            fn deref_mut(&mut self) -> &mut $inner {
                &mut self.0
            }
        }

        impl<$($param,)* S: Default> Default for $name<$($param,)* S> {
            fn default() -> Self {
                Self(<$inner>::default())
            }
        }

        impl<$($param,)* S> Clone for $name<$($param,)* S> where $inner: Clone {
            fn clone(&self) -> Self {
                Self(self.0.clone())
            }
        }

        impl<$($param,)* S> core::fmt::Debug for $name<$($param,)* S> where $inner: core::fmt::Debug {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl<$($param,)* S> PartialEq for $name<$($param,)* S> where $inner: PartialEq {
            fn eq(&self, other: &Self) -> bool {
                self.0 == other.0
            }
        }

        impl<$($param,)* S> Eq for $name<$($param,)* S> where $inner: Eq {}

        impl<$($param,)* S> FromIterator<$item> for $name<$($param,)* S>
        where
            $inner: FromIterator<$item>,
        {
            fn from_iter<I: IntoIterator<Item = $item>>(iter: I) -> Self {
                Self(<$inner>::from_iter(iter))
            }
        }

        impl<$($param,)* S> Extend<$item> for $name<$($param,)* S> where $inner: Extend<$item> {
            fn extend<I: IntoIterator<Item = $item>>(&mut self, iter: I) {
                self.0.extend(iter)
            }
        }

        impl<$($param,)* S> IntoIterator for $name<$($param,)* S> {
            type Item = $item;
            type IntoIter = $iter;
            fn into_iter(self) -> $iter {
                self.0.into_iter()
            }
        }

        impl<'a, $($param,)* S> IntoIterator for &'a $name<$($param,)* S>
        where
            &'a $inner: IntoIterator,
        {
            type Item = <&'a $inner as IntoIterator>::Item;
            type IntoIter = <&'a $inner as IntoIterator>::IntoIter;
            fn into_iter(self) -> Self::IntoIter {
                (&self.0).into_iter()
            }
        }
    };
}

pub mod hash_map {
    //! std's hash map: hashbrown's, keyed with std's hasher by default.

    pub use crate::hash::{DefaultHasher, RandomState};
    pub use hashbrown::hash_map::{
        Drain, IntoIter, IntoKeys, IntoValues, Iter, IterMut, Keys, Values, ValuesMut,
    };

    std_collection!(HashMap<K, V>, hashbrown::HashMap<K, V, S>, (K, V), IntoIter<K, V>);

    impl<'a, K, V, S> IntoIterator for &'a mut HashMap<K, V, S> {
        type Item = (&'a K, &'a mut V);
        type IntoIter = IterMut<'a, K, V>;
        fn into_iter(self) -> IterMut<'a, K, V> {
            self.0.iter_mut()
        }
    }

    impl<K, V, S> HashMap<K, V, S> {
        pub fn into_keys(self) -> IntoKeys<K, V> {
            self.0.into_keys()
        }

        pub fn into_values(self) -> IntoValues<K, V> {
            self.0.into_values()
        }
    }

    impl<K, Q, V, S> core::ops::Index<&Q> for HashMap<K, V, S>
    where
        K: Eq + core::hash::Hash,
        Q: ?Sized + core::hash::Hash + hashbrown::Equivalent<K>,
        S: core::hash::BuildHasher,
    {
        type Output = V;
        fn index(&self, key: &Q) -> &V {
            &self.0[key]
        }
    }

    impl<K: Eq + core::hash::Hash, V, const N: usize> From<[(K, V); N]> for HashMap<K, V> {
        fn from(entries: [(K, V); N]) -> Self {
            entries.into_iter().collect()
        }
    }

    pub type Entry<'a, K, V, S = RandomState> = hashbrown::hash_map::Entry<'a, K, V, S>;
    pub type OccupiedEntry<'a, K, V, S = RandomState> =
        hashbrown::hash_map::OccupiedEntry<'a, K, V, S>;
    pub type VacantEntry<'a, K, V, S = RandomState> = hashbrown::hash_map::VacantEntry<'a, K, V, S>;
}

pub mod hash_set {
    //! std's hash set: hashbrown's, keyed with std's hasher by default.

    pub use hashbrown::hash_set::{Difference, Drain, Intersection, IntoIter, Iter, Union};

    std_collection!(HashSet<T>, hashbrown::HashSet<T, S>, T, IntoIter<T>);

    impl<T: Eq + core::hash::Hash, const N: usize> From<[T; N]> for HashSet<T> {
        fn from(items: [T; N]) -> Self {
            items.into_iter().collect()
        }
    }
}
