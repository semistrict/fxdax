// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: a bucket on the host's filesystem (`file:///dir`, TASK-165.1, backlog doc-14), for
//! development, tests and images: objects are files under a directory, as `object_store`'s
//! [`LocalFileSystem`] keeps them, durably (`fsync` on), with the conditional updates branches rely
//! on (doc-7, Fencing), which `LocalFileSystem` refuses.
//!
//! What replaces or removes an object, an update, an overwrite, a delete, a copy or a rename, holds
//! the lock of the object's directory, an exclusive `flock` of one of [`LOCKS`] files in the
//! bucket's directory, [`lock_of`] the directory names: across processes, and within one, each
//! holding opening the file anew. An update compares the object's e-tag with the one it names
//! under the lock, then writes as an overwrite does, a new file renamed over the object. Creates
//! need no lock: `LocalFileSystem` creates by a hard link, which fails where an object is, and
//! replaces nothing. E-tags are `LocalFileSystem`'s: the file's inode, modification time in
//! microseconds and size. A replaced file's inode can't be the new one's, both existing as the
//! rename goes; one a later write reuses would need the same size and microsecond as the version
//! named, the clock not going back.
//!
//! fxdax: a lock a directory's, not one the bucket's (TASK-395.15): an update holds its lock over
//! its `fsync`, and a collector's passes over many cells at once, each updating its cell's lease a
//! few times, and deleting, waited for one another's on one lock, a sweep at 16 passes at once
//! slower than at 4. Objects of one directory share their lock, so that a batch of deletes, mostly
//! of a few directories, takes a few locks; objects of others mostly don't. Every process of a
//! bucket must lock so: one of a build before, locking `.fxdax-lock` alone, excludes none of them.
//!
//! Deletes in a stream take each lock once for each [`DELETES_LOCKED`] of them ready at once, the
//! objects under one lock deleted several at once under it, as `LocalFileSystem` deletes, and the
//! locks taken at once (TASK-395.5): a collector's batch of deletes takes a lock a directory, not
//! one an object. A delete removes the directories it leaves empty (`LocalFileSystem`'s automatic
//! cleanup), which a listing by delimiter would list as prefixes of nothing, as no other store does,
//! and which piled up before, a directory a branch, for good. A create racing it is unharmed:
//! `LocalFileSystem` makes its directory again where it finds none, and a directory holding its
//! staged file is not empty.

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{StreamExt as _, TryStreamExt as _};
use object_store::local::LocalFileSystem;
use object_store::path::Path;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    ObjectStoreExt as _, PutMode, PutMultipartOptions, PutOptions, PutPayload, PutResult,
    RenameOptions, Result, UpdateVersion,
};
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

/// The start of the names of the files in a bucket's directory whose `flock`s are its locks, each
/// followed by its number, two hexadecimal digits ([`lock_of`]). None is an object: listings leave
/// them out.
pub const LOCK: &str = ".fxdax-lock.";

/// How many locks a bucket has, which the directories of its objects share ([`lock_of`]).
pub const LOCKS: u8 = 64;

/// fxdax: the lock of the objects of the directory of `location`'s (TASK-395.15): the name of its
/// file in the bucket's directory, by a hash of the directory's path, the same in every process
/// and build (FNV-1a), so that each locks an object as the others do.
pub fn lock_of(location: &str) -> String {
    let directory = &location[..location.rfind('/').map_or(0, |slash| slash + 1)];
    let hash = directory.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{LOCK}{:02x}", hash % u64::from(LOCKS))
}

/// The most deletes of a stream one holding of the lock is for.
pub const DELETES_LOCKED: usize = crate::bucket::DELETE_BATCH;

/// A bucket's objects as files under a directory. See the module docs.
#[derive(Clone)]
pub struct Local {
    files: Arc<LocalFileSystem>,
    dir: Arc<std::path::Path>,
}

impl Local {
    /// The objects under `dir`, which is made if missing.
    pub fn new(dir: &std::path::Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let files = LocalFileSystem::new_with_prefix(dir).map_err(std::io::Error::other)?;
        let files = files.with_fsync(true).with_automatic_cleanup(true);
        Ok(Self { files: Arc::new(files), dir: Arc::from(dir) })
    }

    /// The lock `lock` ([`lock_of`]), held until what it returns is dropped: taken on a thread
    /// that may block, if a Tokio runtime has one to give, as `LocalFileSystem` does its I/O.
    async fn lock(&self, lock: String) -> Result<std::fs::File> {
        let path = self.dir.join(lock);
        let shown = path.display().to_string();
        let lock = move || -> std::io::Result<std::fs::File> {
            let file =
                std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path)?;
            file.lock()?;
            Ok(file)
        };
        let locked = match tokio::runtime::Handle::try_current() {
            Ok(runtime) => runtime.spawn_blocking(lock).await.map_err(std::io::Error::other),
            Err(_) => Ok(lock()),
        };
        locked.and_then(|locked| locked).map_err(|source| object_store::Error::Generic {
            store: "Local",
            source: format!("locking {shown}: {source}").into(),
        })
    }

    /// The lock of the object at `location`, held as [`Local::lock`] says.
    async fn lock_for(&self, location: &Path) -> Result<std::fs::File> {
        self.lock(lock_of(location.as_ref())).await
    }

    /// Whether `meta` is an object's, not a lock's.
    fn is_object(meta: &ObjectMeta) -> bool {
        let name = meta.location.as_ref();
        name.contains('/') || !name.starts_with(LOCK)
    }

    /// Deletes `paths`, whose objects' lock is `lock`, under it, held until every delete is
    /// answered: what each came to, each failing as taking the lock did, if it did.
    async fn delete_locked(self, lock: String, paths: Vec<Path>) -> Vec<Result<Path>> {
        match self.lock(lock).await {
            Ok(_locked) => {
                let paths = futures::stream::iter(paths.into_iter().map(Ok)).boxed();
                self.files.delete_stream(paths).collect().await
            }
            Err(failed) => (paths.iter())
                .map(|_| {
                    let source = failed.to_string().into();
                    Err(object_store::Error::Generic { store: "Local", source })
                })
                .collect(),
        }
    }

    /// Writes `payload` at `location` as `opts` say, the lock held: an update only if the object
    /// there is of `version`, then as an overwrite.
    async fn update(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
        version: UpdateVersion,
    ) -> Result<PutResult> {
        let _locked = self.lock_for(location).await?;
        let found = match self.files.head(location).await {
            Ok(meta) => meta.e_tag,
            Err(object_store::Error::NotFound { .. }) => None,
            Err(other) => return Err(other),
        };
        if found.is_none() || found != version.e_tag {
            return Err(object_store::Error::Precondition {
                path: location.to_string(),
                source: format!("{found:?} is not {:?}", version.e_tag).into(),
            });
        }
        let opts = PutOptions { mode: PutMode::Overwrite, ..opts };
        self.files.put_opts(location, payload, opts).await
    }
}

impl fmt::Display for Local {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Local({})", self.dir.display())
    }
}

impl fmt::Debug for Local {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[async_trait]
#[deny(clippy::missing_trait_methods)]
impl ObjectStore for Local {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> Result<PutResult> {
        match opts.mode.clone() {
            PutMode::Create => self.files.put_opts(location, payload, opts).await,
            PutMode::Overwrite => {
                let _locked = self.lock_for(location).await?;
                self.files.put_opts(location, payload, opts).await
            }
            PutMode::Update(version) => self.update(location, payload, opts, version).await,
        }
    }

    /// Completes as an overwrite, as on S3, outside the lock: branches upload nothing by parts,
    /// and nothing completed so is updated.
    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> Result<Box<dyn MultipartUpload>> {
        self.files.put_multipart_opts(location, opts).await
    }

    async fn get_opts(&self, location: &Path, options: GetOptions) -> Result<GetResult> {
        self.files.get_opts(location, options).await
    }

    async fn get_ranges(
        &self,
        location: &Path,
        ranges: &[Range<u64>],
    ) -> Result<Vec<bytes::Bytes>> {
        self.files.get_ranges(location, ranges).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, Result<Path>>,
    ) -> BoxStream<'static, Result<Path>> {
        let bucket = self.clone();
        locations
            .ready_chunks(DELETES_LOCKED)
            .flat_map(move |locations| {
                // Those under one lock together, each lock taken once, the locks at once; what
                // came as an error, as it came.
                let mut locked: std::collections::BTreeMap<String, Vec<Path>> =
                    std::collections::BTreeMap::new();
                let mut failed = Vec::new();
                for location in locations {
                    match location {
                        Ok(path) => locked.entry(lock_of(path.as_ref())).or_default().push(path),
                        Err(error) => failed.push(Err(error)),
                    }
                }
                let bucket = bucket.clone();
                let deleted = futures::stream::iter(locked)
                    .map(move |(lock, paths)| bucket.clone().delete_locked(lock, paths))
                    .buffer_unordered(usize::from(LOCKS))
                    .flat_map(futures::stream::iter);
                futures::stream::iter(failed).chain(deleted)
            })
            .boxed()
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, Result<ObjectMeta>> {
        let listed = self.files.list(prefix);
        listed.try_filter(|meta| std::future::ready(Self::is_object(meta))).boxed()
    }

    fn list_with_offset(
        &self,
        prefix: Option<&Path>,
        offset: &Path,
    ) -> BoxStream<'static, Result<ObjectMeta>> {
        let listed = self.files.list_with_offset(prefix, offset);
        listed.try_filter(|meta| std::future::ready(Self::is_object(meta))).boxed()
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> Result<ListResult> {
        let mut listed = self.files.list_with_delimiter(prefix).await?;
        listed.objects.retain(Self::is_object);
        Ok(listed)
    }

    async fn copy_opts(&self, from: &Path, to: &Path, options: CopyOptions) -> Result<()> {
        let _locked = self.lock_for(to).await?;
        self.files.copy_opts(from, to, options).await
    }

    /// Under the locks of both, the first by their names taken first, as every holding of both
    /// takes them.
    async fn rename_opts(&self, from: &Path, to: &Path, options: RenameOptions) -> Result<()> {
        let mut locks = [lock_of(from.as_ref()), lock_of(to.as_ref())];
        locks.sort();
        let _first = self.lock(locks[0].clone()).await?;
        let _second = match locks[0] == locks[1] {
            true => None,
            false => Some(self.lock(locks[1].clone()).await?),
        };
        self.files.rename_opts(from, to, options).await
    }
}

#[cfg(test)]
mod tests {
    use super::{LOCK, LOCKS, lock_of};
    use crate::bucket::{Bucket as _, Error, PutMode};
    use crate::store::tests::file;

    /// Updates of one version, many at once through two buckets of one directory, as two
    /// processes' would be: one wins, whose bytes the object holds, and the others find it changed.
    #[test]
    fn one_update_of_a_version_wins() {
        let dir = tempfile::tempdir().unwrap();
        let runtime =
            tokio::runtime::Builder::new_multi_thread().worker_threads(8).build().unwrap();
        runtime.block_on(async {
            let buckets = [file(dir.path()), file(dir.path())];
            let key = "leases/0000000000000001";
            let version =
                buckets[0].put(key, b"first".to_vec().into(), PutMode::Create).await.unwrap();
            let updates = (0..32u8).map(|n| {
                let (bucket, version) = (buckets[usize::from(n % 2)].clone(), version.clone());
                tokio::spawn(async move {
                    (
                        n,
                        bucket
                            .put(key, vec![n; 64].into(), PutMode::Update(version))
                            .await
                            .map(|_| ()),
                    )
                })
            });
            let results = futures::future::try_join_all(updates).await.unwrap();
            let won: Vec<u8> =
                results.iter().filter(|(_, put)| put.is_ok()).map(|(n, _)| *n).collect();
            let lost = results.iter().filter(|(_, put)| *put == Err(Error::Precondition)).count();
            assert_eq!((won.len(), lost), (1, 31));
            assert_eq!(buckets[1].get(key).await.unwrap().bytes, vec![won[0]; 64]);
        });
    }

    /// Objects outlive the bucket that wrote them, read by another of their directory, made if
    /// missing; the lock is in the directory and lists as no object.
    #[test]
    fn objects_outlive_their_bucket_and_the_lock_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().join("made/here");
        futures::executor::block_on(async {
            let bucket = file(&dir);
            let version = bucket.put("a", vec![1].into(), PutMode::Create).await.unwrap();
            bucket.put("a", vec![2].into(), PutMode::Update(version)).await.unwrap();
            bucket.put("b", vec![3].into(), PutMode::Create).await.unwrap();
            bucket.delete("b").await.unwrap();
        });
        assert!(dir.join(lock_of("a")).exists());
        futures::executor::block_on(async {
            let bucket = file(&dir);
            let listed: Vec<String> =
                bucket.list("").await.unwrap().into_iter().map(|meta| meta.key).collect();
            assert_eq!(listed, ["a"]);
            let level = bucket.list_delimited("", crate::bucket::Page::FIRST).await.unwrap();
            let listed: Vec<String> = level.objects.into_iter().map(|meta| meta.key).collect();
            assert_eq!((level.prefixes, listed), (vec![], vec!["a".to_owned()]));
            assert_eq!(bucket.get("a").await.unwrap().bytes, [2]);
        });
    }

    /// TASK-395.15: the objects of a directory share a lock, by the directory's path alone, the
    /// same in every build; those of others mostly don't.
    #[test]
    fn objects_of_a_directory_share_their_lock() {
        let cell = "tenants/0000000000000001/cells/0000000000000002/";
        let lease = format!("{cell}gc/lease");
        assert_eq!(lock_of(&lease), ".fxdax-lock.21");
        let segment = |seq: u64| format!("{cell}segments/0000000000000001/{seq:016x}");
        assert_eq!(lock_of(&segment(1)), lock_of(&segment(2)));
        assert_eq!(lock_of("a"), lock_of("b"));
        let leases: std::collections::BTreeSet<String> = (1..=16u64)
            .map(|id| lock_of(&format!("tenants/0000000000000001/cells/{id:016x}/gc/lease")))
            .collect();
        // A sweep's 16 passes at once over a tenant's first cells, each updating its lease: no
        // two of them under one lock.
        assert_eq!(leases.len(), 16, "{leases:?}");
        let every: std::collections::BTreeSet<String> =
            (0..4096u64).map(|n| lock_of(&format!("{n:x}/object"))).collect();
        assert_eq!(every.len(), usize::from(LOCKS));
        assert!(every.iter().all(|lock| lock.starts_with(LOCK) && lock.len() == LOCK.len() + 2));
    }

    /// TASK-395.15: an update goes while another holding, as another process's, holds the lock of
    /// another directory: updates of objects of different directories wait for none of each
    /// other's, as before they all waited for the bucket's one lock. One of an object under the
    /// held lock goes once it is let go.
    #[test]
    fn updates_wait_only_for_their_own_lock() {
        let dir = tempfile::tempdir().unwrap();
        let runtime =
            tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap();
        let (held, free) = ("held/object", "free/object");
        assert_ne!(lock_of(held), lock_of(free));
        runtime.block_on(async {
            let bucket = file(dir.path());
            let mut versions = Vec::new();
            for key in [held, free] {
                versions.push(bucket.put(key, vec![1].into(), PutMode::Create).await.unwrap());
            }
            let lock = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(dir.path().join(lock_of(held)))
                .unwrap();
            lock.lock().unwrap();
            let waiting = tokio::spawn({
                let (bucket, version) = (bucket.clone(), versions[0].clone());
                async move { bucket.put(held, vec![2].into(), PutMode::Update(version)).await }
            });
            bucket.put(free, vec![2].into(), PutMode::Update(versions[1].clone())).await.unwrap();
            assert!(!waiting.is_finished());
            lock.unlock().unwrap();
            waiting.await.unwrap().unwrap();
            assert_eq!(bucket.get(held).await.unwrap().bytes, [2]);
        });
    }
}
