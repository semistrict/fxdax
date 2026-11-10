// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: a bucket on the host's filesystem (`file:///dir`, TASK-165.1, backlog doc-14), for
//! development, tests and images: objects are files under a directory, as `object_store`'s
//! [`LocalFileSystem`] keeps them, durably (`fsync` on), with the conditional updates branches rely
//! on (doc-7, Fencing), which `LocalFileSystem` refuses.
//!
//! What replaces or removes an object, an update, an overwrite, a delete, a copy or a rename, holds
//! the bucket's lock, an exclusive `flock` of [`LOCK`] in its directory: across processes, and
//! within one, each holding opening the file anew. An update compares the object's e-tag with the
//! one it names under the lock, then writes as an overwrite does, a new file renamed over the
//! object. Creates need no lock: `LocalFileSystem` creates by a hard link, which fails where an
//! object is, and replaces nothing. E-tags are `LocalFileSystem`'s: the file's inode, modification
//! time in microseconds and size. A replaced file's inode can't be the new one's, both existing as
//! the rename goes; one a later write reuses would need the same size and microsecond as the
//! version named, the clock not going back.

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

/// The file in a bucket's directory whose `flock` is the bucket's lock. It is no object: listings
/// leave it out.
pub const LOCK: &str = ".fxdax-lock";

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
        Ok(Self { files: Arc::new(files.with_fsync(true)), dir: Arc::from(dir) })
    }

    /// The bucket's lock, held until what it returns is dropped: taken on a thread that may block,
    /// if a Tokio runtime has one to give, as `LocalFileSystem` does its I/O.
    async fn lock(&self) -> Result<std::fs::File> {
        let path = self.dir.join(LOCK);
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
            source: format!("locking {}: {source}", self.dir.join(LOCK).display()).into(),
        })
    }

    /// Whether `meta` is an object's, not the lock's.
    fn is_object(meta: &ObjectMeta) -> bool {
        meta.location.as_ref() != LOCK
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
        let _locked = self.lock().await?;
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
                let _locked = self.lock().await?;
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
            .and_then(move |location| {
                let bucket = bucket.clone();
                async move {
                    let _locked = bucket.lock().await?;
                    bucket.files.delete(&location).await?;
                    Ok(location)
                }
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
        let _locked = self.lock().await?;
        self.files.copy_opts(from, to, options).await
    }

    async fn rename_opts(&self, from: &Path, to: &Path, options: RenameOptions) -> Result<()> {
        let _locked = self.lock().await?;
        self.files.rename_opts(from, to, options).await
    }
}

#[cfg(test)]
mod tests {
    use super::LOCK;
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
        assert!(dir.join(LOCK).exists());
        futures::executor::block_on(async {
            let bucket = file(&dir);
            let listed: Vec<String> =
                bucket.list("").await.unwrap().into_iter().map(|meta| meta.key).collect();
            assert_eq!(listed, ["a"]);
            assert_eq!(bucket.get("a").await.unwrap().bytes, [2]);
        });
    }
}
