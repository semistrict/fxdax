// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::object_record::{AttributeId, AttributeKey, ObjectKey, ObjectKeyData, ObjectValue};
use crate::lsm_tree::cache::{ObjectCache, ObjectCachePlaceholder, ObjectCacheResult};
use fuchsia_sync::{Mutex, MutexGuard};
use linked_hash_map::{Entry, LinkedHashMap};
use std::hash::{BuildHasher, BuildHasherDefault};
use std::sync::atomic::{AtomicU64, Ordering};

fn filter(key: &ObjectKey) -> bool {
    match key.data {
        // Attribute and keys could also be added here to some immediate benefit, but would be
        // somewhat redundant with a node cache planned to be added after.
        ObjectKeyData::Object => true,
        ObjectKeyData::ExtendedAttribute { .. } => true,
        ObjectKeyData::Keys => true,
        // fxdax: names in directories neither encrypted nor case-insensitive, which lookups find
        // exactly (`Directory::lookup_ext`), and the record of an object's data, which says its
        // size: a file's attributes are its object's and its data's. That record goes with its
        // object's (`invalidate`).
        ObjectKeyData::Child { .. } => true,
        ObjectKeyData::Attribute(AttributeId::DATA, AttributeKey::Attribute) => true,
        _ => false,
    }
}

// Limiting to ~100KiB of space usage. 56 bytes of linear overhead per item plus the overhead of
// the structure. fxdax: the default, which a filesystem's options can change
// (`Options::object_cache_items`).
pub(crate) const ITEM_LIMIT: usize = 1535;

/// fxdax: how many shards, each with a lock and least-recently-used order of its own, a cache
/// larger than `SHARD_ABOVE` items is split into, by its keys' hashes: threads changing a store at
/// once otherwise all take one lock, around every lookup and change of its tree. A cache no larger
/// is one, as upstream's.
const SHARDS: usize = 16;
const SHARD_ABOVE: usize = 16 * ITEM_LIMIT;

type Shard = LinkedHashMap<ObjectKey, CacheValue, BuildHasherDefault<rustc_hash::FxHasher>>;

struct Placeholder<'a> {
    cache: &'a TreeCache,
    key: Option<ObjectKey>,
    placeholder_id: u64,
}

impl Placeholder<'_> {
    fn replace_entry(&mut self, value: Option<CacheValue>) {
        let key = self.key.take().expect("This method should only be called once");
        let mut inner = self.cache.shard(&key).lock();
        // The value is present...
        if let Entry::Occupied(mut entry) = inner.entry(key) {
            // And the same placeholder as the token has...
            let is_current = match entry.get() {
                CacheValue::Placeholder(placeholder_id) => &self.placeholder_id == placeholder_id,
                _ => false,
            };
            if is_current {
                match value {
                    Some(v) => *(entry.get_mut()) = v,
                    None => {
                        entry.remove();
                    }
                }
            }
        }
    }
}

impl Drop for Placeholder<'_> {
    fn drop(&mut self) {
        if self.key.is_some() {
            self.replace_entry(None);
        }
    }
}

impl<'a> ObjectCachePlaceholder<ObjectValue> for Placeholder<'a> {
    fn complete(mut self: Box<Self>, value: Option<&ObjectValue>) {
        let entry_value = match value {
            value @ Some(ObjectValue::Object { .. }) => value.cloned(),
            value @ Some(ObjectValue::ExtendedAttribute(_)) => value.cloned(),
            value @ Some(ObjectValue::Keys(_)) => value.cloned(),
            value @ Some(ObjectValue::Child(_)) => value.cloned(),
            value @ Some(ObjectValue::Attribute { .. }) => value.cloned(),
            value @ Some(ObjectValue::VerifiedAttribute { .. }) => value.cloned(),
            // fxdax: there is none.
            value @ Some(ObjectValue::None) => value.cloned(),
            _ => None,
        }
        .map(|v| CacheValue::Value(v));
        self.replace_entry(entry_value);
    }
}

enum CacheValue {
    Placeholder(u64),
    Value(ObjectValue),
}

/// Supports caching for Objects directly for now. Speeds up stat calls.
pub struct TreeCache {
    /// fxdax: the cache's shards (`SHARDS`), each holding at most `limit` items.
    shards: Box<[Mutex<Shard>]>,
    placeholder_counter: AtomicU64,
    limit: usize,
}

impl TreeCache {
    /// fxdax: a cache of at most `limit` items (`ITEM_LIMIT` by default), which grows to that as
    /// it fills.
    pub fn with_limit(limit: usize) -> Self {
        let shards = if limit > SHARD_ABOVE { SHARDS } else { 1 };
        let limit = limit.div_ceil(shards);
        Self {
            shards: (0..shards)
                .map(|_| {
                    Mutex::new(LinkedHashMap::with_capacity_and_hasher(
                        limit.min(ITEM_LIMIT) + 1,
                        BuildHasherDefault::<rustc_hash::FxHasher>::default(),
                    ))
                })
                .collect(),
            placeholder_counter: AtomicU64::new(0),
            limit,
        }
    }

    /// The shard `key` is in.
    fn shard(&self, key: &ObjectKey) -> &Mutex<Shard> {
        let hash = BuildHasherDefault::<rustc_hash::FxHasher>::default().hash_one(key);
        &self.shards[hash as usize % self.shards.len()]
    }
}

impl ObjectCache<ObjectKey, ObjectValue> for TreeCache {
    fn lookup_or_reserve(&self, key: &ObjectKey) -> ObjectCacheResult<'_, ObjectValue> {
        if !filter(key) {
            return ObjectCacheResult::NoCache;
        }
        let inner = self.shard(key).lock();
        match MutexGuard::try_map_or_err(inner, |inner| match inner.get_refresh(key) {
            Some(CacheValue::Value(v)) => Ok(v),
            Some(CacheValue::Placeholder(_)) => Err(false),
            // fxdax: a name not cached isn't reserved: the kernel caches what its lookups of
            // names find, and what they don't (dentries), and names are cached as they are
            // written (`invalidate`). A lookup of a missing name would make and drop a placeholder.
            None => Err(!matches!(key.data, ObjectKeyData::Child { .. })),
        }) {
            Ok(guard) => ObjectCacheResult::Value(guard),
            Err((_, false)) => ObjectCacheResult::NoCache,
            Err((mut inner, true)) => {
                let placeholder_id = self.placeholder_counter.fetch_add(1, Ordering::Relaxed);
                inner.insert(key.clone(), CacheValue::Placeholder(placeholder_id));
                if inner.len() > self.limit {
                    let _ = inner.pop_front();
                }
                ObjectCacheResult::Placeholder(Box::new(Placeholder {
                    cache: self,
                    key: Some(key.clone()),
                    placeholder_id,
                }))
            }
        }
    }

    fn is_cacheable(&self, key: &ObjectKey) -> bool {
        filter(key)
    }

    fn invalidate(&self, key: &ObjectKey, value: Option<ObjectValue>) {
        if !filter(key) {
            return;
        }
        // fxdax: an object's record removed, or deleted, takes its other records with it, by
        // merging (a tombstone): its data's record goes from the cache too.
        if key.data == ObjectKeyData::Object
            && value.as_ref().is_none_or(|value| *value == ObjectValue::None)
        {
            let data =
                ObjectKey::attribute(key.object_id, AttributeId::DATA, AttributeKey::Attribute);
            self.shard(&data).lock().remove(&data);
        }
        let mut inner = self.shard(key).lock();
        if let Some(replacement) = value {
            if let Some(entry) = inner.get_mut(key) {
                *entry = CacheValue::Value(replacement);
            } else {
                // fxdax: a record written is cached too, as the kernel's caches keep what it
                // creates: the object a file just created is, say, which its unlink finds.
                inner.insert(key.clone(), CacheValue::Value(replacement));
                if inner.len() > self.limit {
                    let _ = inner.pop_front();
                }
            }
        } else {
            inner.remove(key);
        }
    }

    fn clear(&self) {
        for shard in &self.shards {
            shard.lock().clear();
        }
    }

    fn len(&self) -> usize {
        self.shards.iter().map(|shard| shard.lock().len()).sum()
    }

    /// fxdax: a shard at a time, round about, each's least recently used first, so that each
    /// shrinks alike. A placeholder removed is completed to nothing (`Placeholder::replace_entry`).
    fn shrink(&self, count: usize) -> usize {
        let mut removed = 0;
        while removed < count {
            let before = removed;
            for shard in &self.shards {
                if removed == count {
                    break;
                }
                if shard.lock().pop_front().is_some() {
                    removed += 1;
                }
            }
            if removed == before {
                break;
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::super::object_record::{
        AttributeId, AttributeKey, DirType, ObjectDescriptor, ObjectKey, ObjectValue, Timestamp,
    };
    use super::{ITEM_LIMIT, TreeCache};
    use crate::lsm_tree::cache::{ObjectCache, ObjectCacheResult};
    use crate::object_store::EncryptionKey;
    use assert_matches::assert_matches;

    #[fuchsia::test]
    async fn test_basic_operations() {
        let cache = TreeCache::with_limit(ITEM_LIMIT);
        let key = ObjectKey::object(1);
        let now = Timestamp::now();
        let value = ObjectValue::file(1, 0, now, now, now, now, None, None);

        let placeholder = match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder,
            _ => panic!("Expected cache miss with placeholder returned."),
        };
        placeholder.complete(Some(&value));

        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &value),
            _ => panic!("Expected to find item."),
        };

        cache.invalidate(&key, None);

        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder.complete(None),
            _ => panic!("Expected cache miss with placeholder returned."),
        };
    }

    #[fuchsia::test]
    async fn test_enforce_limits() {
        enforce_limit(ITEM_LIMIT);
    }

    /// fxdax: a cache larger than the default keeps as many items as it was made for.
    #[fuchsia::test]
    async fn test_enforce_larger_limit() {
        enforce_limit(4 * ITEM_LIMIT);
    }

    /// fxdax: a large cache, in shards, keeps at most as many items as it was made for, and what
    /// was used last.
    #[fuchsia::test]
    async fn test_sharded_cache_limit() {
        let limit = 2 * super::SHARD_ABOVE;
        let cache = TreeCache::with_limit(limit);
        assert_eq!(cache.shards.len(), super::SHARDS);
        let now = Timestamp::now();
        let value = ObjectValue::file(1, 0, now, now, now, now, None, None);
        for i in 0..(2 * limit as u64) {
            cache.invalidate(&ObjectKey::object(i), Some(value.clone()));
        }
        // Each shard is full: it took more keys than it holds.
        assert_eq!(cache.len(), limit);
        let last = ObjectKey::object(2 * limit as u64 - 1);
        assert_matches!(cache.lookup_or_reserve(&last), ObjectCacheResult::Value(_));
    }

    /// fxdax: shrinking removes what was used least recently, and stops when the cache is empty;
    /// a cache in shards shrinks by as many.
    #[fuchsia::test]
    async fn test_shrink() {
        let now = Timestamp::now();
        let value = ObjectValue::file(1, 0, now, now, now, now, None, None);
        for limit in [ITEM_LIMIT, 2 * super::SHARD_ABOVE] {
            let cache = TreeCache::with_limit(limit);
            for i in 0..1000 {
                cache.invalidate(&ObjectKey::object(i), Some(value.clone()));
            }
            assert_eq!(cache.shrink(900), 900);
            assert_eq!(cache.len(), 100);
            assert_eq!(cache.shrink(1000), 100);
            assert_eq!(cache.len(), 0);
        }
        // In one shard: what was used again, the most recently used, is left.
        let cache = TreeCache::with_limit(ITEM_LIMIT);
        for i in 0..1000 {
            cache.invalidate(&ObjectKey::object(i), Some(value.clone()));
        }
        assert_matches!(cache.lookup_or_reserve(&ObjectKey::object(0)), ObjectCacheResult::Value(_));
        assert_eq!(cache.shrink(999), 999);
        assert_matches!(cache.lookup_or_reserve(&ObjectKey::object(0)), ObjectCacheResult::Value(_));
    }

    fn enforce_limit(limit: usize) {
        let cache = TreeCache::with_limit(limit);
        let now = Timestamp::now();

        for i in 1..(limit as u64 + 2) {
            let key = ObjectKey::object(i);
            let value = ObjectValue::file(1, 0, now, now, now, now, None, None);
            let placeholder = match cache.lookup_or_reserve(&key) {
                ObjectCacheResult::Placeholder(placeholder) => placeholder,
                _ => panic!("Expected cache miss with placeholder returned."),
            };
            placeholder.complete(Some(&value));
        }

        // Item 1 should be evicted.
        assert_matches!(
            cache.lookup_or_reserve(&ObjectKey::object(1)),
            ObjectCacheResult::Placeholder(_)
        );

        // And item 2 has been evicted by the lookup of item 1.
        for i in 3..(limit as u64 + 2) {
            let key = ObjectKey::object(i);
            assert_matches!(cache.lookup_or_reserve(&key), ObjectCacheResult::Value(_));
        }
    }

    #[fuchsia::test]
    async fn test_invalidate_inserts() {
        let cache = TreeCache::with_limit(ITEM_LIMIT);
        let key = ObjectKey::object(1);
        let now = Timestamp::now();
        let value1 = ObjectValue::file(1, 0, now, now, now, now, None, None);
        let value2 = ObjectValue::file(2, 0, now, now, now, now, None, None);

        let placeholder = match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder,
            _ => panic!("Expected cache miss with placeholder returned."),
        };
        placeholder.complete(Some(&value1));

        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &value1),
            _ => panic!("Expected to find item."),
        };

        cache.invalidate(&key, Some(value2.clone()));

        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &value2),
            _ => panic!("Expected to find item."),
        };
    }

    #[fuchsia::test]
    async fn test_no_caching_for_filtered_item() {
        let cache = TreeCache::with_limit(ITEM_LIMIT);
        let key = ObjectKey::extent(1, AttributeId::TEST_ID, 1..2);

        assert!(!cache.is_cacheable(&key));
        assert!(matches!(cache.lookup_or_reserve(&key), ObjectCacheResult::NoCache));
    }

    #[fuchsia::test]
    async fn test_is_cacheable() {
        let cache = TreeCache::with_limit(ITEM_LIMIT);

        // Cacheable keys: Object, ExtendedAttribute, Keys, and fxdax: attributes' records
        assert!(cache.is_cacheable(&ObjectKey::object(1)));
        assert!(cache.is_cacheable(&ObjectKey::keys(1)));
        assert!(cache.is_cacheable(&ObjectKey::extended_attribute(1, vec![1, 2, 3])));
        assert!(cache.is_cacheable(&ObjectKey::attribute(
            1,
            AttributeId::DATA,
            AttributeKey::Attribute
        )));

        // Non-cacheable keys: Extent, Graveyard, etc.
        assert!(!cache.is_cacheable(&ObjectKey::extent(1, AttributeId::TEST_ID, 1..2)));
        assert!(!cache.is_cacheable(&ObjectKey::graveyard_entry(1, 2)));
        assert!(!cache.is_cacheable(&ObjectKey::graveyard_attribute_entry(
            1,
            2,
            AttributeId::TEST_ID
        )));
    }

    // Two clients looking for the same key don't interfere with each other. Prevents priority
    // inversion.
    #[fuchsia::test]
    async fn test_two_parallel_clients() {
        let cache = TreeCache::with_limit(ITEM_LIMIT);
        let key = ObjectKey::object(1);
        let now = Timestamp::now();
        let value1 = ObjectValue::file(1, 0, now, now, now, now, None, None);
        let value2 = ObjectValue::file(2, 0, now, now, now, now, None, None);

        let placeholder1 = match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder,
            _ => panic!("Expected cache miss with placeholder returned."),
        };

        // Another search should not get a placeholder, as one is already held.
        assert!(matches!(cache.lookup_or_reserve(&key), ObjectCacheResult::NoCache));

        // Invalidate the current placeholder.
        cache.invalidate(&key, None);

        // Get a new placeholder
        let placeholder2 = match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder,
            _ => panic!("Expected cache miss with placeholder returned."),
        };

        // Complete them out of order.
        placeholder2.complete(Some(&value2));
        placeholder1.complete(Some(&value1));

        // Result should be from the second placeholder, as the first was invalidated.
        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &value2),
            _ => panic!("Expected to find item."),
        };
    }

    #[fuchsia::test]
    async fn test_keys_are_cached() {
        let cache = TreeCache::with_limit(ITEM_LIMIT);
        let key = ObjectKey::keys(1);
        let value = ObjectValue::keys(
            vec![(2, EncryptionKey::FscryptInoLblk32File { key_identifier: [3u8; 16] })].into(),
        );

        let placeholder = match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder,
            _ => panic!("Expected cache miss with placeholder returned."),
        };
        placeholder.complete(Some(&value));

        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &value),
            _ => panic!("Expected to find item."),
        };

        cache.invalidate(&key, None);

        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder.complete(None),
            _ => panic!("Expected cache miss with placeholder returned."),
        };
    }

    /// fxdax: a name in a directory neither encrypted nor case-insensitive is cached as it is
    /// written, not as a lookup misses it, and forgotten once removed; others aren't, which
    /// lookups don't find exactly.
    #[fuchsia::test]
    async fn test_children_are_cached() {
        let cache = TreeCache::with_limit(ITEM_LIMIT);
        let key = ObjectKey::child(1, "name", DirType::Normal);
        let value = ObjectValue::child(2, ObjectDescriptor::File);

        assert_matches!(cache.lookup_or_reserve(&key), ObjectCacheResult::NoCache);
        cache.invalidate(&key, Some(value.clone()));
        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &value),
            _ => panic!("Expected to find item."),
        };

        // Removing the name marks it deleted, which a lookup finds as nothing.
        cache.invalidate(&key, Some(ObjectValue::None));
        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &ObjectValue::None),
            _ => panic!("Expected to find the deleted marker."),
        };

        assert!(!cache.is_cacheable(&ObjectKey::child(1, "name", DirType::Casefold)));
        assert!(!cache.is_cacheable(&ObjectKey::encrypted_child(1, vec![1, 2, 3], None)));
    }

    /// fxdax: that there is no item is cached too, until one is inserted.
    #[fuchsia::test]
    async fn test_absence_is_cached() {
        let cache = TreeCache::with_limit(ITEM_LIMIT);
        let key = ObjectKey::extended_attribute(1, b"user.name".to_vec());

        let placeholder = match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder,
            _ => panic!("Expected cache miss with placeholder returned."),
        };
        placeholder.complete(Some(&ObjectValue::None));
        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &ObjectValue::None),
            _ => panic!("Expected to find that there is none."),
        };

        let value = ObjectValue::inline_extended_attribute(b"value".to_vec());
        cache.invalidate(&key, Some(value.clone()));
        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &value),
            _ => panic!("Expected to find the item inserted."),
        };
    }

    /// fxdax: an object's tombstone, which deletes its other records by merging, takes its
    /// data's record from the cache too.
    #[fuchsia::test]
    async fn test_object_tombstone_takes_its_data_record() {
        let cache = TreeCache::with_limit(ITEM_LIMIT);
        let key = ObjectKey::attribute(1, AttributeId::DATA, AttributeKey::Attribute);
        let value = ObjectValue::attribute(4096, false);
        let placeholder = match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder,
            _ => panic!("Expected cache miss with placeholder returned."),
        };
        placeholder.complete(Some(&value));
        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &value),
            _ => panic!("Expected to find item."),
        };

        cache.invalidate(&ObjectKey::object(1), None);
        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder.complete(None),
            _ => panic!("Expected the data's record to be gone."),
        };
    }

    /// fxdax: a record written is cached, though not looked up before.
    #[fuchsia::test]
    async fn test_written_records_are_cached() {
        let cache = TreeCache::with_limit(ITEM_LIMIT);
        let key = ObjectKey::object(1);
        let now = Timestamp::now();
        let value = ObjectValue::file(1, 0, now, now, now, now, None, None);
        cache.invalidate(&key, Some(value.clone()));
        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &value),
            _ => panic!("Expected to find the record written."),
        };
    }

    #[fuchsia::test]
    async fn test_dropped_placeholder_clears_entry() {
        let cache = TreeCache::with_limit(ITEM_LIMIT);
        let key = ObjectKey::object(1);

        {
            let _placeholder = match cache.lookup_or_reserve(&key) {
                ObjectCacheResult::Placeholder(placeholder) => placeholder,
                _ => panic!("Expected cache miss with placeholder returned."),
            };
        }

        assert_matches!(cache.lookup_or_reserve(&key), ObjectCacheResult::Placeholder(_));
    }

    #[fuchsia::test]
    async fn test_clear() {
        let cache = TreeCache::with_limit(ITEM_LIMIT);
        let key = ObjectKey::object(1);
        let now = Timestamp::now();
        let value = ObjectValue::file(1, 0, now, now, now, now, None, None);

        if let ObjectCacheResult::Placeholder(placeholder) = cache.lookup_or_reserve(&key) {
            placeholder.complete(Some(&value));
        } else {
            panic!("Expected cache miss with placeholder returned.");
        }

        assert_matches!(cache.lookup_or_reserve(&key), ObjectCacheResult::Value(_));
        cache.clear();
        assert_matches!(cache.lookup_or_reserve(&key), ObjectCacheResult::Placeholder(_));
    }
}
