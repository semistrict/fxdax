// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An image's store kept in its bucket (backlog doc-14, TASK-165.6): as every store's branch is,
//! in the bucket its description names, an image's own file bucket where it was made on no host
//! of fxdax's system (`host_store::create`). An uploader takes the branch's lease and uploads the
//! image's commits behind them, as the extent daemon's stores' are (`crate::remote`); the image's
//! commits are durable once in its store, as before, and closing it waits for them to be uploaded.
//! A clone is a fork of the branch in the bucket, as the daemon's forks are (`host_store::fork`).
//! An image's own bucket's collector runs in the process, after commits ([`super::collect`]); a
//! host's bucket's is the host's.

use super::collect::{self, Collecting};
use crate::error::{Error, anyhow_of};
use crate::host_store::{self, Opened};
use crate::remote;
use extent_remote::layout::Child;
use extent_store::upload::Remote;
use extent_store::{HostDir, Store};
use std::path::Path;
use std::sync::Arc;

/// An image's store, its branch kept in its bucket. See the module docs.
pub(crate) struct Kept {
    store: Arc<Store<HostDir>>,
    /// The branch's uploader, which opened the store: dropped, it closes the store's open segment,
    /// uploads what the store made durable, then stops (TASK-192).
    remote: Remote<HostDir>,
    /// What reads the store's extents whose data is in the bucket.
    fetcher: Arc<remote::Fetcher>,
    /// The bucket's collector in this process, if it is the image's own, which no host's collector
    /// collects.
    collecting: Option<Arc<Collecting>>,
}

impl Kept {
    /// Makes a new image at `dir`, which must not exist, holding at most `capacity` bytes, of
    /// `branch` ([`host_store::create`]): whole if `whole`, or else unmade until its maker says it
    /// is ([`host_store::create_unmade`], TASK-196), as an import's, once filled.
    pub fn create(dir: &Path, capacity: u64, branch: u64, whole: bool) -> anyhow::Result<Self> {
        let keeping = host_store::keeping().map_err(anyhow_of)?;
        let opened = host_store::create_unmade(dir, capacity, branch, None, keeping);
        let opened = opened.map_err(anyhow_of)?;
        if whole {
            opened.made()?;
        }
        Self::opened(opened)
    }

    /// The image at `dir`, or the clone whose handle `dir` is: its uploader takes its branch's
    /// lease as a copy never placed takes it where this process is no host's daemon
    /// ([`host_store::keeping`]).
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        let keeping = host_store::keeping().map_err(anyhow_of)?;
        Self::opened(host_store::open(dir, keeping).map_err(anyhow_of)?)
    }

    /// `opened`, its branch taken in its bucket.
    fn opened(opened: Opened) -> anyhow::Result<Self> {
        let Opened { store, bucket, remote } = opened;
        let url = store.bucket().unwrap_or_default().to_owned();
        let fetcher = remote::Fetcher::new(bucket.clone(), None);
        let own = host_store::keeps_own_bucket(&store);
        let collecting = own.then(|| collect::of(&url, &bucket));
        Ok(Self { store, remote, fetcher, collecting })
    }

    pub fn store(&self) -> &Arc<Store<HostDir>> {
        &self.store
    }

    /// What reads the store's extents whose data is in the bucket, as a clone's inherited extents
    /// are (TASK-165.7).
    pub fn fetcher(&self) -> Arc<remote::Fetcher> {
        self.fetcher.clone()
    }

    /// What follows a commit of the store's: it fails once the uploader has, the branch another
    /// writer's or deleted, or its bucket out of reach for good, rather than the image go on as
    /// a branch no bucket has; it waits while more of its commits wait to be uploaded than the
    /// lag's bound; and the bucket's collector runs, if it is due.
    pub fn committed(&self) -> anyhow::Result<()> {
        if let Some(failed) = self.remote.failed() {
            return Err(anyhow::Error::new(failed).context("the image's branch is not uploaded"));
        }
        self.remote.wait_lag(self.store.epoch()?)?;
        if let Some(collecting) = &self.collecting {
            collecting.after_commit();
        }
        Ok(())
    }

    /// Waits until the store's commits are uploaded: so a closed image's branch is whole in its
    /// bucket.
    pub fn wait_uploaded(&self) -> anyhow::Result<()> {
        Ok(self.remote.wait_uploaded(self.store.epoch()?)?)
    }

    /// Forks the store's branch, as of its last commit, to a new branch, whose handle is a new
    /// directory at `path`, in its bucket too ([`host_store::fork`]): `held` is let go of once the
    /// fork's commit is made.
    pub fn fork<H>(&self, path: &Path, held: H) -> Result<(), Error> {
        let child = Child { branch: host_store::new_branch(), owner: rand::random(), point: false };
        host_store::fork(&self.store, &self.remote, path, child, |_| Ok(()), held)
    }

    /// The bucket's collector in this process, if it is the image's own.
    #[cfg(test)]
    pub fn collecting(&self) -> Option<&Arc<Collecting>> {
        self.collecting.as_ref()
    }
}

/// Deletes the image at `path`, or the clone whose handle `path` is, which nothing in this process
/// has open ([`host_store::delete`]): its branch, in the bucket its description names, first, as
/// a copy never placed deletes it where this process is no host's daemon ([`host_store::token`]).
pub(crate) fn delete(path: &Path) -> Result<(), Error> {
    let bucket = host_store::bucket_at(path).map_err(Error::new)?;
    host_store::delete(path, bucket.as_deref(), host_store::token)
}

#[cfg(test)]
mod tests {
    use super::super::{Kind, Storage};
    use super::*;
    use crate::{Filesystem, Owner};
    use extent_remote::record::Kind as Record;
    use extent_remote::store::Store as Bucket;
    use futures::executor::block_on;
    use std::collections::BTreeMap;
    use storage_device::ExtentId;

    const OWNER: Owner = Owner { uid: 1000, gid: 100 };
    const SIZE: u64 = 64 << 20;
    const MIB: usize = 1 << 20;

    /// The image `storage` is of.
    fn image(storage: &Storage) -> &Arc<Kept> {
        let Kind::Image { image, .. } = &storage.kind else { panic!("memory") };
        image
    }

    /// A formatted image at `path`, holding `file` with `contents`, closed.
    async fn image_with(path: &Path, file: &[u8], contents: &[u8]) -> Storage {
        let fs = Filesystem::format(Storage::create(path, SIZE).unwrap()).await.unwrap();
        let (node, _) = fs.root().create(file, 0o644, OWNER).await.unwrap();
        node.write(0, contents).await.unwrap();
        fs.close().await.unwrap()
    }

    /// The bucket the store at `path` names.
    fn bucket_of(path: &Path) -> Arc<Bucket> {
        host_store::bucket_at(path).unwrap().expect("a bucket")
    }

    /// The extents `branch`'s state in `bucket` holds, by their lengths, those it inherited too,
    /// and its root.
    fn kept(bucket: &Bucket, branch: u64) -> (BTreeMap<ExtentId, u64>, Vec<u8>) {
        let recovered = extent_store::upload::block_on(async {
            let record = extent_remote::branch::record(bucket, branch).await?;
            extent_remote::recover(bucket, branch, record.and_then(|record| record.origin())).await
        });
        let state = recovered.unwrap().unwrap().state;
        let extents = state.extents.iter().map(|(extent, at)| (*extent, at.len)).collect();
        (extents, state.root.unwrap_or_default())
    }

    /// The extents `image`'s store holds, by their lengths, and its root.
    fn held(image: &Kept) -> (BTreeMap<ExtentId, u64>, Vec<u8>) {
        let store = image.store();
        (store.extents().unwrap().into_iter().collect(), store.root().unwrap())
    }

    /// The bytes of the files of the bucket in the directory `dir`.
    fn bucket_bytes(dir: &Path) -> u64 {
        let entries = std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap());
        (entries)
            .map(|entry| match entry.file_type().unwrap().is_dir() {
                true => bucket_bytes(&entry.path()),
                false => entry.metadata().unwrap().len(),
            })
            .sum()
    }

    /// TASK-165.6: an image keeps its branch in a file bucket of its own, beside its directory
    /// (TASK-165.7), which its description names: closed, the branch holds what its store does.
    #[test]
    fn images_keep_their_branches_in_a_file_bucket_of_their_own() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image");
        let storage = block_on(image_with(&path, b"file", b"contents"));
        let own = dir.path().canonicalize().unwrap().join("image.bucket");
        let url = format!("file://{}/", own.display());
        assert_eq!(extent_store::host_bucket(&path).unwrap(), Some(url));
        let image = image(&storage);
        let branch = image.store().description().branch;
        let (extents, root) = kept(&bucket_of(&path), branch);
        assert!(!extents.is_empty() && !root.is_empty());
        assert_eq!((extents, root), held(image));
    }

    /// TASK-165.6: a clone is a fork of its image's branch in the image's bucket, where it keeps
    /// its own: a branch of its own there, whose record names the image's as its parent, and which
    /// holds what the clone's store does, closed.
    #[test]
    fn clones_are_forks_in_their_images_bucket() {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let (path, clone_path) = (dir.path().join("image"), dir.path().join("clone"));
            let storage = image_with(&path, b"file", b"original").await;
            let parent = image(&storage).store().description().branch;
            let clone = storage.clone_to(&clone_path).await.unwrap();
            let child = image(&clone).store().description().branch;
            let bucket = bucket_of(&clone_path);
            assert!(Arc::ptr_eq(&bucket, &bucket_of(&path)), "the clone's bucket is another");
            let record = extent_remote::branch::record(&*bucket, child).await.unwrap().unwrap();
            let Record::Fork(origin) = record.kind else { panic!("{record:?}") };
            assert_eq!(origin.parent, parent);

            let fs = Filesystem::mount(clone).await.unwrap();
            let (file, _) = fs.root().lookup(b"file").await.unwrap();
            file.write(0, b"modified").await.unwrap();
            let clone = fs.close().await.unwrap();
            assert_eq!(kept(&bucket, child), held(image(&clone)));
            assert_ne!(kept(&bucket, child), kept(&bucket, parent));
        })
    }

    /// TASK-165.6: deleting an image, or a clone, deletes its branch in its bucket too, which the
    /// collector then reclaims what only it held of.
    #[test]
    fn deleting_images_deletes_their_branches() {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let (path, clone_path) = (dir.path().join("image"), dir.path().join("clone"));
            let storage = image_with(&path, b"file", b"original").await;
            let clone = storage.clone_to(&clone_path).await.unwrap();
            let child = image(&clone).store().description().branch;
            drop((clone, storage));
            let bucket = bucket_of(&path);
            let record = |branch| extent_remote::branch::record(&*bucket, branch);
            assert!(!record(child).await.unwrap().unwrap().deleted);
            Storage::delete(&clone_path).unwrap();
            assert!(record(child).await.unwrap().unwrap().deleted);
        })
    }

    /// TASK-165.6: an image's commit starts a pass of its bucket's collector, on a thread of its
    /// own, which the image's clones share; the next commits, within a minute of it, start none.
    #[test]
    fn commits_start_the_collector() {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("image");
            let storage = Storage::create(&path, SIZE).unwrap();
            let collecting = image(&storage).collecting().expect("its own bucket's").clone();
            assert_eq!(collecting.passes(), 0);
            let fs = Filesystem::format(storage).await.unwrap();
            assert_eq!(collecting.passes(), 1);
            let (node, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            node.write(0, b"contents").await.unwrap();
            let storage = fs.close().await.unwrap();
            assert_eq!(collecting.passes(), 1);
            let clone = storage.clone_to(dir.path().join("clone")).await.unwrap();
            let shared = image(&clone).collecting().unwrap();
            assert!(Arc::ptr_eq(shared, &collecting), "the clone's collector is another");
        })
    }

    /// TASK-165.6: an image dropped has let go of its store, which this process opens again at
    /// once, as the extent daemon does a store an import made: its uploader's task, held up by the
    /// failpoint after it said it stopped, keeps nothing of it open. That task said so before it let
    /// go of the store, and the daemon found the import's store still open (`EIO`, CI 42119298585).
    #[test]
    fn images_dropped_let_go_of_their_stores() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image");
        let scenario = fail::FailScenario::setup();
        fail::cfg("remote::stopped", "sleep(500)").unwrap();
        drop(Storage::create(&path, SIZE).unwrap());
        let reopened = host_store::open(&path, host_store::keeping().unwrap())
            .map(|opened| opened.store.description().branch);
        scenario.teardown();
        let branch = reopened.map_err(|err| format!("{err:#}"));
        assert_eq!(branch.map(|_| ()), Ok(()));
    }

    /// TASK-165.6: an image kept in a host's bucket, as a store the extent daemon made is, which
    /// this process opens, has no collector here: the host's collects it.
    #[test]
    fn images_in_a_hosts_bucket_are_not_collected_here() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store");
        let keeping = host_store::keeping().unwrap();
        let made = host_store::create_unmade(&path, SIZE, 7, Some("memory:///host"), keeping);
        made.unwrap().made().unwrap();
        let image = Kept::open(&path).unwrap();
        assert!(image.collecting().is_none());
        image.store().write(ExtentId::new(7, 1), 0, &[1; 4096]).unwrap();
        image.store().commit(None).unwrap();
        image.committed().unwrap();
        image.wait_uploaded().unwrap();
        assert_eq!(kept(&bucket_of(&path), 7).0, BTreeMap::from([(ExtentId::new(7, 1), 4096)]));
    }

    /// TASK-165.6: what an image freed, uploaded, and checkpointed as its uploader stops, a pass of
    /// its bucket's collector reclaims; what it holds, the branch still holds.
    #[test]
    fn the_collector_reclaims_what_images_freed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image");
        let image = Kept::create(&path, SIZE, 7, true).unwrap();
        let extent = |sequence| ExtentId::new(7, sequence);
        // Each stored on its own, in its bucket: 5 MiB, past the size for it.
        let data = crate::test_data::noise(1, 5 * MIB);
        for sequence in 0..4 {
            image.store().write(extent(sequence), 0, &data).unwrap();
        }
        image.store().commit(None).unwrap();
        image.committed().unwrap();
        // Uploaded before they are freed: a segment of the commits sealing and freeing them both
        // would hold none of them.
        image.wait_uploaded().unwrap();
        for sequence in 0..3 {
            image.store().free(extent(sequence)).unwrap();
        }
        image.store().commit(Some(b"root")).unwrap();
        image.committed().unwrap();
        image.wait_uploaded().unwrap();
        drop(image);

        let own = dir.path().join("image.bucket");
        let before = bucket_bytes(&own);
        assert!(before >= 20 * MIB as u64, "{before} bytes");
        let image = Kept::open(&path).unwrap();
        let report = image.collecting().unwrap().pass_now();
        assert!(report.deleted >= 3, "{report:?}");
        let after = bucket_bytes(&own);
        assert!(after < 6 * MIB as u64, "{after} bytes");
        let (extents, root) = kept(&bucket_of(&path), 7);
        assert_eq!(
            (extents, root),
            (BTreeMap::from([(extent(3), 5 * MIB as u64)]), b"root".into())
        );
    }
}
