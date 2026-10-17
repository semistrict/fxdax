// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A bucket of S3 or GCS, or of anything else the `object_store` crate reaches, under a prefix.
//!
//! Branches rely on conditional writes (doc-7, Fencing): a create where an object is must fail,
//! and an update of a version that is not the object's. `object_store` sends S3's `If-None-Match`
//! and `If-Match`, and GCS's generation preconditions; [`probe`] checks that a store honours them
//! before anything relies on it.

use crate::bucket::{Bucket, ByteRange, Error, Meta, Object, PutMode, Version};
use futures::{StreamExt as _, TryStreamExt as _, stream};
use object_store::path::Path;
use object_store::{
    GetOptions, GetRange, ObjectMeta, ObjectStore, PutOptions, PutPayload, UpdateVersion,
};
use std::future::Future;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// A bucket's objects under a prefix.
pub struct Store {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

impl Store {
    /// The objects of `store` under `prefix`, a path of segments separated by `/`, or empty.
    pub fn new(store: Arc<dyn ObjectStore>, prefix: &str) -> Self {
        Self { store, prefix: prefix.trim_matches('/').to_owned() }
    }

    /// The bucket at `url`: `s3://bucket/prefix`, `gs://bucket/prefix`, or `memory:///prefix`,
    /// configured, credentials included, from the environment's `AWS_*` or `GOOGLE_*` variables.
    pub fn from_url(url: &str) -> Result<Self, String> {
        let url = url::Url::parse(url).map_err(|error| format!("{url}: {error}"))?;
        let (store, path) = object_store::parse_url_opts(&url, std::env::vars())
            .map_err(|error| format!("{url}: {error}"))?;
        Ok(Self::new(Arc::from(store), path.as_ref()))
    }

    fn path(&self, key: &str) -> Path {
        if self.prefix.is_empty() {
            Path::from(key)
        } else {
            Path::from(format!("{}/{key}", self.prefix))
        }
    }

    fn meta(&self, meta: ObjectMeta) -> Meta {
        let location = meta.location.as_ref();
        let key = match self.prefix.is_empty() {
            true => location,
            false => location
                .strip_prefix(&self.prefix)
                .map_or(location, |key| key.trim_start_matches('/')),
        };
        Meta {
            key: key.to_owned(),
            version: Version { e_tag: meta.e_tag, version: meta.version },
            size: meta.size,
            modified: meta.last_modified.timestamp_millis().max(0) as u64,
        }
    }
}

/// What a request's failure means to branches. Failures other than the ones the protocol answers
/// are retried: the store may be unreachable for a while, and branches wait (doc-7, decision 3).
fn error(error: object_store::Error) -> Error {
    match error {
        object_store::Error::NotFound { .. } => Error::NotFound,
        object_store::Error::AlreadyExists { .. } => Error::AlreadyExists,
        object_store::Error::Precondition { .. } | object_store::Error::NotModified { .. } => {
            Error::Precondition
        }
        _ => Error::Unavailable,
    }
}

impl Bucket for Store {
    fn get(&self, key: &str) -> impl Future<Output = Result<Object, Error>> + Send {
        let (store, path) = (self.store.clone(), self.path(key));
        async move {
            let result = store.get_opts(&path, GetOptions::default()).await.map_err(error)?;
            let meta = self.meta(result.meta.clone());
            let bytes = result.bytes().await.map_err(error)?;
            Ok(Object { bytes: bytes.to_vec(), meta })
        }
    }

    fn get_range(
        &self,
        key: &str,
        range: ByteRange,
    ) -> impl Future<Output = Result<Vec<u8>, Error>> + Send {
        let (store, path) = (self.store.clone(), self.path(key));
        let range = match range {
            ByteRange::Bounded(range) => GetRange::Bounded(range),
            ByteRange::Suffix(n) => GetRange::Suffix(n),
        };
        async move {
            let options = GetOptions { range: Some(range), ..GetOptions::default() };
            let result = store.get_opts(&path, options).await.map_err(error)?;
            Ok(result.bytes().await.map_err(error)?.to_vec())
        }
    }

    fn head(&self, key: &str) -> impl Future<Output = Result<Meta, Error>> + Send {
        let (store, path) = (self.store.clone(), self.path(key));
        async move {
            let options = GetOptions { head: true, ..GetOptions::default() };
            let result = store.get_opts(&path, options).await.map_err(error)?;
            Ok(self.meta(result.meta))
        }
    }

    fn list(&self, prefix: &str) -> impl Future<Output = Result<Vec<Meta>, Error>> + Send {
        // Prefixes are whole segments: `object_store` lists by them.
        assert!(prefix.is_empty() || prefix.ends_with('/'), "a prefix of keys: {prefix}");
        let store = self.store.clone();
        let path = match (self.prefix.is_empty(), prefix.trim_end_matches('/')) {
            (true, "") => None,
            (_, prefix) => Some(self.path(prefix)),
        };
        async move {
            let listed: Vec<ObjectMeta> =
                store.list(path.as_ref()).try_collect().await.map_err(error)?;
            let mut metas: Vec<Meta> = listed.into_iter().map(|meta| self.meta(meta)).collect();
            metas.sort_by(|a, b| a.key.cmp(&b.key));
            Ok(metas)
        }
    }

    fn put(
        &self,
        key: &str,
        bytes: Vec<u8>,
        mode: PutMode,
    ) -> impl Future<Output = Result<Version, Error>> + Send {
        let (store, path) = (self.store.clone(), self.path(key));
        let mode = match mode {
            PutMode::Overwrite => object_store::PutMode::Overwrite,
            PutMode::Create => object_store::PutMode::Create,
            PutMode::Update(Version { e_tag, version }) => {
                object_store::PutMode::Update(UpdateVersion { e_tag, version })
            }
        };
        async move {
            let options = PutOptions { mode, ..PutOptions::default() };
            let result =
                store.put_opts(&path, PutPayload::from(bytes), options).await.map_err(error)?;
            Ok(Version { e_tag: result.e_tag, version: result.version })
        }
    }

    fn delete(&self, key: &str) -> impl Future<Output = Result<(), Error>> + Send {
        let (store, path) = (self.store.clone(), self.path(key));
        async move {
            let mut deleted = store.delete_stream(stream::once(async move { Ok(path) }).boxed());
            match deleted.next().await {
                Some(Err(object_store::Error::NotFound { .. })) | Some(Ok(_)) | None => Ok(()),
                Some(Err(other)) => Err(error(other)),
            }
        }
    }

    fn now(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64)
    }

    fn pause(&self, failures: u32) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(backoff(failures))
    }
}

/// How long a request that failed `failures` times in a row waits before it goes again: from 50 ms,
/// doubling, to 5 s.
fn backoff(failures: u32) -> std::time::Duration {
    std::time::Duration::from_millis((50u64 << failures.saturating_sub(1).min(7)).min(5_000))
}

/// Checks that `bucket` honours the conditional writes branches rely on, with an object of its
/// own at `probe/<nonce>`, which it removes: a create where an object is fails, so does an update
/// of a version that is not the object's, and one of the object's version succeeds.
pub async fn probe(bucket: &impl Bucket, nonce: u64) -> Result<(), String> {
    let key = format!("probe/{nonce:016x}");
    let refused = |what: &str, result: &dyn std::fmt::Debug| {
        format!("the store does not honour conditional writes: {what} gave {result:?}")
    };
    let created = bucket.put(&key, b"probe".to_vec(), PutMode::Create).await;
    let version = created.map_err(|error| format!("creating {key}: {error}"))?;
    let again = bucket.put(&key, b"again".to_vec(), PutMode::Create).await;
    if again != Err(Error::AlreadyExists) {
        return Err(refused("a create where an object is", &again));
    }
    // No store makes this ETag; GCS's generation 0 is no object's.
    let other =
        Version { e_tag: Some("\"fxdax-probe\"".to_owned()), version: Some("0".to_owned()) };
    let stale = bucket.put(&key, b"stale".to_vec(), PutMode::Update(other)).await;
    if stale != Err(Error::Precondition) {
        return Err(refused("an update of another version", &stale));
    }
    let updated = bucket.put(&key, b"updated".to_vec(), PutMode::Update(version)).await;
    if updated.is_err() {
        return Err(refused("an update of the object's version", &updated));
    }
    bucket.delete(&key).await.map_err(|error| format!("removing {key}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Requests that failed wait from 50 ms, doubling, to at most 5 s.
    #[test]
    fn failed_requests_back_off() {
        let waits: Vec<_> = (1..=10).map(|failures| backoff(failures).as_millis()).collect();
        assert_eq!(waits, [50, 100, 200, 400, 800, 1600, 3200, 5000, 5000, 5000]);
    }
    use crate::bucket::Bucket;
    use crate::layout::Origin;
    use crate::writer::{Commit, Writer};
    use crate::{Error as BranchError, branch, collector, reader};
    use object_store::memory::InMemory;
    use storage_device::ExtentId;

    fn memory() -> Arc<Store> {
        Arc::new(Store::new(Arc::new(InMemory::new()), "fxdax/test"))
    }

    #[test]
    fn honours_conditional_writes() {
        futures::executor::block_on(probe(&*memory(), 1)).unwrap();
    }

    #[test]
    fn keys_are_under_the_prefix() {
        let store = memory();
        futures::executor::block_on(async {
            for key in ["segments/01/00", "segments/01/01", "segments/02/00", "leases/01"] {
                store.put(key, key.as_bytes().to_vec(), PutMode::Create).await.unwrap();
            }
            let listed: Vec<String> = store
                .list("segments/01/")
                .await
                .unwrap()
                .into_iter()
                .map(|meta| meta.key)
                .collect();
            assert_eq!(listed, ["segments/01/00", "segments/01/01"]);
            assert_eq!(store.list("").await.unwrap().len(), 4);
            let object = store.get("leases/01").await.unwrap();
            assert_eq!(
                (object.bytes.as_slice(), object.meta.key.as_str()),
                (&b"leases/01"[..], "leases/01")
            );
            assert_eq!(store.get("leases/02").await.map(|_| ()), Err(Error::NotFound));
            store.delete("leases/01").await.unwrap();
            store.delete("leases/01").await.unwrap();
            assert_eq!(store.head("leases/01").await.map(|_| ()), Err(Error::NotFound));
        });
    }

    /// A branch's life over `bucket`: written, forked, freed, checkpointed, taken over, collected.
    pub(crate) async fn branch_lifecycle(bucket: Arc<impl Bucket>, root: u64) {
        let child = root + 1;
        branch::create(&*bucket, root).await.unwrap();
        let first = Writer::open(bucket.clone(), root, 10).await.unwrap();
        let small = ExtentId::new(root, first.next_sequence());
        let large = ExtentId::new(root, small.sequence + 1);
        let commit = Commit {
            tag: 1,
            extents: vec![(small, b"small".to_vec())],
            own: vec![(large, vec![7; 4096])],
            ..Commit::default()
        };
        first.commit(commit).unwrap();
        let seq = first.close().unwrap();
        first.upload(seq).await.unwrap();
        assert_eq!(first.acknowledge().await, Ok(Some(seq)));
        let fork = first.fork(child).unwrap();
        first.upload(fork).await.unwrap();
        first.acknowledge().await.unwrap();
        first.commit(Commit { tag: 2, frees: vec![small], ..Commit::default() }).unwrap();
        let seq = first.close().unwrap();
        first.upload(seq).await.unwrap();
        first.acknowledge().await.unwrap();
        assert_eq!(first.checkpoint().await, Ok(Some(seq)));
        // Looked up a block at a time, by ranged reads.
        let checkpoint = crate::layout::checkpoint(root, seq);
        let table = crate::checkpoint::Table::open(&*bucket, &checkpoint).await.unwrap().unwrap();
        assert_eq!(table.state().last_commit, Some(2));
        assert!(table.lookup(&*bucket, large).await.unwrap().is_some());
        assert_eq!(table.lookup(&*bucket, small).await, Ok(None));

        // Another writer takes the branch; the first one's next segment is not the branch's.
        let second = Writer::open(bucket.clone(), root, 11).await.unwrap();
        first.commit(Commit { tag: 3, frees: vec![large], ..Commit::default() }).unwrap();
        let seq = first.close().unwrap();
        let taken = match first.upload(seq).await {
            Ok(()) => first.acknowledge().await.map(|_| ()),
            Err(error) => Err(error),
        };
        assert_eq!(taken, Err(BranchError::Fenced));
        assert_eq!(second.state().last_commit, Some(2));

        let state = reader::recover(&*bucket, root, None).await.unwrap().state;
        assert_eq!(state.extents.keys().copied().collect::<Vec<_>>(), [large]);
        let origin = Some(Origin { parent: root, at: fork });
        assert_eq!(branch::record(&*bucket, child).await.unwrap().unwrap().origin, origin);
        let inherited = reader::recover(&*bucket, child, origin).await.unwrap().state;
        assert_eq!(inherited.extents.keys().copied().collect::<Vec<_>>(), [small, large]);

        // The collector keeps what the child inherited, though the parent freed it.
        let options = collector::Options { compact_below: 0.5, grace: 0 };
        collector::collect(&*bucket, 20, &options).await.unwrap();
        let location = &inherited.extents[&small];
        assert_eq!(reader::read(&*bucket, small, location).await.unwrap(), b"small");
        let location = &inherited.extents[&large];
        assert_eq!(reader::read(&*bucket, large, location).await.unwrap(), vec![7; 4096]);
    }

    #[test]
    fn branches_on_a_memory_store() {
        futures::executor::block_on(branch_lifecycle(memory(), 1));
    }

    /// The same over a real S3 server: `scripts/test-rustfs.sh` starts RustFS and runs this with
    /// `FXDAX_S3_URL` naming a bucket of it, and the `AWS_*` variables to reach it.
    #[test]
    #[ignore]
    fn rustfs() {
        let url = std::env::var("FXDAX_S3_URL").expect("FXDAX_S3_URL names the bucket to use");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let store = Arc::new(Store::from_url(&url).unwrap());
            probe(&*store, std::process::id().into()).await.unwrap();
            // Each run its own branches.
            let root = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;
            branch_lifecycle(store, root & !1).await;
        });
    }
}
