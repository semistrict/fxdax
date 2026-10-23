// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A bucket of S3 or GCS, or of anything else the `object_store` crate reaches, under a prefix.
//!
//! Branches rely on conditional writes (doc-7, Fencing): a create where an object is must fail,
//! and an update of a version that is not the object's. `object_store` sends S3's `If-None-Match`
//! and `If-Match`, and GCS's generation preconditions; [`probe`] checks that a store honours them
//! before anything relies on it, once for each store ([`Store::probed`]).
//!
//! A store counts the requests it makes, by kind, those that fail too, which object storage bills,
//! and the bytes it uploads, by kind of object, and downloads ([`Store::stats`]).

use crate::bucket::{Bucket, ByteRange, Error, Meta, Object, PutMode, Version};
use crate::layout::{self, Kind};
use futures::{StreamExt as _, TryStreamExt as _, stream};
use object_store::path::Path;
use object_store::{
    GetOptions, GetRange, ObjectMeta, ObjectStore, PutOptions, PutPayload, UpdateVersion,
};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A bucket's objects under a prefix.
pub struct Store {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    counts: Counts,
    /// Whether [`Store::probed`] found it honours conditional writes: what it found holds.
    honoured: std::sync::atomic::AtomicBool,
}

/// The kinds of request a store makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Request {
    Get,
    /// A GET of a range of an object.
    GetRange,
    Head,
    List,
    /// A PUT where no object is.
    Create,
    /// A PUT of an object's version.
    Update,
    Overwrite,
    Delete,
}

impl Request {
    pub const ALL: [Self; 8] = [
        Self::Get,
        Self::GetRange,
        Self::Head,
        Self::List,
        Self::Create,
        Self::Update,
        Self::Overwrite,
        Self::Delete,
    ];

    /// Its name in [`Stats::line`].
    pub fn name(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::GetRange => "get-range",
            Self::Head => "head",
            Self::List => "list",
            Self::Create => "create",
            Self::Update => "update",
            Self::Overwrite => "overwrite",
            Self::Delete => "delete",
        }
    }
}

/// The kinds of object a store uploads, by their keys (doc-7, Layout).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Upload {
    Segments,
    /// Extents stored on their own.
    Extents,
    Checkpoints,
    /// The collector's moves of what it compacted.
    Moves,
    /// Branches' records and leases, and whatever else.
    Other,
}

impl Upload {
    pub const ALL: [Self; 5] =
        [Self::Segments, Self::Extents, Self::Checkpoints, Self::Moves, Self::Other];

    fn of(key: &str) -> Self {
        match layout::kind(key) {
            Kind::Segment { .. } => Self::Segments,
            Kind::Extent { .. } => Self::Extents,
            Kind::Checkpoint { .. } => Self::Checkpoints,
            Kind::Moves { .. } => Self::Moves,
            _ => Self::Other,
        }
    }

    /// Its name in [`Stats::line`].
    pub fn name(self) -> &'static str {
        match self {
            Self::Segments => "segments",
            Self::Extents => "extents",
            Self::Checkpoints => "checkpoints",
            Self::Moves => "moves",
            Self::Other => "other",
        }
    }
}

#[derive(Default)]
struct Counts {
    requests: [AtomicU64; Request::ALL.len()],
    uploaded: [AtomicU64; Upload::ALL.len()],
    downloaded: AtomicU64,
}

/// What a store did so far: its requests by kind, the bytes it uploaded by kind of object, and the
/// bytes it downloaded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub requests: [u64; Request::ALL.len()],
    pub uploaded: [u64; Upload::ALL.len()],
    pub downloaded: u64,
}

impl Stats {
    pub fn requests(&self, request: Request) -> u64 {
        self.requests[request as usize]
    }

    pub fn uploaded(&self, upload: Upload) -> u64 {
        self.uploaded[upload as usize]
    }

    /// The stats as one line, which `scripts/bench-report.py` reads from the extent daemon's log:
    /// `object-store:`, then `NAME=COUNT` for each kind of request, `up-NAME=BYTES` for each kind
    /// of object, and `down=BYTES`.
    pub fn line(&self) -> String {
        let mut line = "object-store:".to_owned();
        for request in Request::ALL {
            line += &format!(" {}={}", request.name(), self.requests(request));
        }
        for upload in Upload::ALL {
            line += &format!(" up-{}={}", upload.name(), self.uploaded(upload));
        }
        line + &format!(" down={}", self.downloaded)
    }
}

impl Counts {
    fn request(&self, request: Request) {
        self.requests[request as usize].fetch_add(1, Ordering::Relaxed);
    }
}

impl Store {
    /// The objects of `store` under `prefix`, a path of segments separated by `/`, or empty.
    pub fn new(store: Arc<dyn ObjectStore>, prefix: &str) -> Self {
        let prefix = prefix.trim_matches('/').to_owned();
        Self { store, prefix, counts: Counts::default(), honoured: Default::default() }
    }

    /// Checks that it honours the conditional writes branches rely on ([`probe`]), with an object
    /// of `nonce`'s, unless it found it does before: a host's stores share their bucket, which it
    /// checks once, at their first attach, not at each (doc-7, Fencing).
    pub async fn probed(&self, nonce: u64) -> Result<(), String> {
        if !self.honoured.load(Ordering::Relaxed) {
            probe(self, nonce).await?;
            self.honoured.store(true, Ordering::Relaxed);
        }
        Ok(())
    }

    /// What it did so far.
    pub fn stats(&self) -> Stats {
        let load = |count: &AtomicU64| count.load(Ordering::Relaxed);
        Stats {
            requests: self.counts.requests.each_ref().map(load),
            uploaded: self.counts.uploaded.each_ref().map(load),
            downloaded: load(&self.counts.downloaded),
        }
    }

    /// The bucket at `url`: `s3://bucket/prefix`, `gs://bucket/prefix`, `az://container/prefix`
    /// (Azure Blob Storage, TASK-46), or `memory:///prefix`, configured, credentials included, from
    /// the environment's `AWS_*`, `GOOGLE_*` or `AZURE_*` variables: for Azure, the account
    /// (`AZURE_STORAGE_ACCOUNT_NAME`) and its key (`AZURE_STORAGE_ACCOUNT_KEY`), a SAS token, or a
    /// service principal's or managed identity's, as `object_store`'s `MicrosoftAzureBuilder`
    /// reads them; the emulator, Azurite, with `AZURE_STORAGE_USE_EMULATOR=true`. Blob Storage's
    /// conditional writes (If-None-Match: *, If-Match on ETags) are those doc-7 relies on.
    pub fn from_url(url: &str) -> Result<Self, String> {
        let (store, prefix) = object_store_at(url)?;
        Ok(Self::new(store, &prefix))
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

/// The object store at `url`, as [`Store::from_url`] reaches it, and the prefix the URL names in
/// it: for tests that put themselves between a store and its objects. Each of its requests waits
/// FXDAX_REMOTE_LATENCY_MS milliseconds more, if set: an object store as far as a host's own may
/// be, as the sandbox team's RustFS, 25 ms away, which made Fxfs's commits wait (TASK-61, TASK-62).
pub fn object_store_at(url: &str) -> Result<(Arc<dyn ObjectStore>, String), String> {
    let url = url::Url::parse(url).map_err(|error| format!("{url}: {error}"))?;
    let (store, path) = object_store::parse_url_opts(&url, std::env::vars())
        .map_err(|error| format!("{url}: {error}"))?;
    let latency = match std::env::var("FXDAX_REMOTE_LATENCY_MS") {
        Ok(ms) if !ms.is_empty() => std::time::Duration::from_millis(
            ms.parse().map_err(|_| format!("FXDAX_REMOTE_LATENCY_MS={ms} is no count"))?,
        ),
        _ => std::time::Duration::ZERO,
    };
    if latency.is_zero() {
        return Ok((Arc::from(store), path.as_ref().to_owned()));
    }
    let config = object_store::throttle::ThrottleConfig {
        wait_delete_per_call: latency,
        wait_get_per_call: latency,
        wait_list_per_call: latency,
        wait_list_with_delimiter_per_call: latency,
        wait_put_per_call: latency,
        ..Default::default()
    };
    let throttled = object_store::throttle::ThrottledStore::new(store, config);
    Ok((Arc::new(throttled), path.as_ref().to_owned()))
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
            self.counts.request(Request::Get);
            let result = store.get_opts(&path, GetOptions::default()).await.map_err(error)?;
            let meta = self.meta(result.meta.clone());
            let bytes = result.bytes().await.map_err(error)?;
            self.counts.downloaded.fetch_add(bytes.len() as u64, Ordering::Relaxed);
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
            self.counts.request(Request::GetRange);
            let options = GetOptions { range: Some(range.clone()), ..GetOptions::default() };
            let result = match store.get_opts(&path, options).await {
                // A store that reads no suffix of an object (Azure Blob Storage, TASK-46), which it
                // says before any request: the object's size first, then the range it ends with.
                Err(object_store::Error::NotSupported { .. })
                    if matches!(range, GetRange::Suffix(_)) =>
                {
                    let GetRange::Suffix(len) = range else { unreachable!() };
                    self.counts.request(Request::Head);
                    let head = GetOptions { head: true, ..GetOptions::default() };
                    let size = store.get_opts(&path, head).await.map_err(error)?.meta.size;
                    let range = GetRange::Bounded(size.saturating_sub(len)..size);
                    let options = GetOptions { range: Some(range), ..GetOptions::default() };
                    store.get_opts(&path, options).await.map_err(error)?
                }
                result => result.map_err(error)?,
            };
            let bytes = result.bytes().await.map_err(error)?;
            self.counts.downloaded.fetch_add(bytes.len() as u64, Ordering::Relaxed);
            Ok(bytes.to_vec())
        }
    }

    fn head(&self, key: &str) -> impl Future<Output = Result<Meta, Error>> + Send {
        let (store, path) = (self.store.clone(), self.path(key));
        async move {
            self.counts.request(Request::Head);
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
            // A listing of many objects takes a request for each page of them (1,000 on S3 and
            // GCS), which `object_store` does not say: counted as one.
            self.counts.request(Request::List);
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
        let (request, mode) = match mode {
            PutMode::Overwrite => (Request::Overwrite, object_store::PutMode::Overwrite),
            PutMode::Create => (Request::Create, object_store::PutMode::Create),
            PutMode::Update(Version { e_tag, version }) => {
                (Request::Update, object_store::PutMode::Update(UpdateVersion { e_tag, version }))
            }
        };
        let upload = Upload::of(key);
        async move {
            self.counts.request(request);
            let len = bytes.len() as u64;
            let options = PutOptions { mode, ..PutOptions::default() };
            let result =
                store.put_opts(&path, PutPayload::from(bytes), options).await.map_err(error)?;
            self.counts.uploaded[upload as usize].fetch_add(len, Ordering::Relaxed);
            Ok(Version { e_tag: result.e_tag, version: result.version })
        }
    }

    fn delete(&self, key: &str) -> impl Future<Output = Result<(), Error>> + Send {
        let (store, path) = (self.store.clone(), self.path(key));
        async move {
            self.counts.request(Request::Delete);
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
    use crate::batch::{Batch, Commit};
    use crate::bucket::Bucket;
    use crate::layout::Origin;
    use crate::writer::Writer;
    use crate::{Error as BranchError, branch, collector, recovery};
    use object_store::memory::InMemory;
    use storage_device::ExtentId;

    fn memory() -> Arc<Store> {
        Arc::new(Store::new(Arc::new(InMemory::new()), "fxdax/test"))
    }

    /// FXDAX_REMOTE_LATENCY_MS adds its milliseconds to each request, as a farther object store's
    /// would (TASK-62): a put and a get, two of them.
    #[test]
    fn requests_wait_the_latency_set() {
        // SAFETY: nextest runs each test in a process of its own, whose other threads don't read
        // the environment yet.
        unsafe { std::env::set_var("FXDAX_REMOTE_LATENCY_MS", "50") };
        let store = Store::from_url("memory:///latency").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let start = std::time::Instant::now();
        runtime.block_on(async {
            store.put("a", vec![1], PutMode::Create).await.unwrap();
            assert_eq!(store.get("a").await.unwrap().bytes, vec![1]);
        });
        let took = start.elapsed();
        assert!(took >= std::time::Duration::from_millis(100), "two requests took {took:?}");
    }

    /// A store counts its requests by kind, failed ones too, and the bytes it uploads, by kind of
    /// object, and downloads.
    #[test]
    fn counts_requests_and_bytes() {
        let store = memory();
        futures::executor::block_on(async {
            let segment = layout::segment(1, 0);
            let version = store.put(&segment, vec![1; 100], PutMode::Create).await.unwrap();
            assert!(store.put(&segment, vec![2; 7], PutMode::Create).await.is_err());
            store.put(&segment, vec![3; 50], PutMode::Update(version)).await.unwrap();
            store.put(&layout::checkpoint(1, 0), vec![4; 30], PutMode::Create).await.unwrap();
            store.put(&layout::lease(1), vec![5; 9], PutMode::Overwrite).await.unwrap();
            let extent = layout::extent(1, 0, storage_device::ExtentId::new(1, 5));
            store.put(&extent, vec![6; 11], PutMode::Create).await.unwrap();
            store.put(&layout::moves(&segment), vec![7; 13], PutMode::Create).await.unwrap();
            assert_eq!(store.get(&segment).await.unwrap().bytes.len(), 50);
            let range = ByteRange::Bounded(10..30);
            assert_eq!(store.get_range(&segment, range).await.unwrap().len(), 20);
            assert!(store.get(&layout::segment(1, 9)).await.is_err());
            store.head(&segment).await.unwrap();
            store.list("segments/").await.unwrap();
            store.delete(&segment).await.unwrap();
        });
        let stats = store.stats();
        let requests: Vec<_> =
            Request::ALL.iter().map(|request| (request.name(), stats.requests(*request))).collect();
        assert_eq!(
            requests,
            [
                ("get", 2),
                ("get-range", 1),
                ("head", 1),
                ("list", 1),
                ("create", 5),
                ("update", 1),
                ("overwrite", 1),
                ("delete", 1),
            ]
        );
        assert_eq!(stats.uploaded, [150, 11, 30, 13, 9]);
        assert_eq!(stats.downloaded, 70);
        assert_eq!(
            stats.line(),
            "object-store: get=2 get-range=1 head=1 list=1 create=5 update=1 overwrite=1 delete=1 \
             up-segments=150 up-extents=11 up-checkpoints=30 up-moves=13 up-other=9 down=70"
        );
    }

    /// A bucket that writes whatever it is asked to, conditions or not.
    struct Careless(crate::sim::bucket::Direct);

    impl Bucket for Careless {
        fn get(&self, key: &str) -> impl Future<Output = Result<Object, Error>> + Send {
            self.0.get(key)
        }

        fn get_range(
            &self,
            key: &str,
            range: ByteRange,
        ) -> impl Future<Output = Result<Vec<u8>, Error>> + Send {
            self.0.get_range(key, range)
        }

        fn head(&self, key: &str) -> impl Future<Output = Result<Meta, Error>> + Send {
            self.0.head(key)
        }

        fn list(&self, prefix: &str) -> impl Future<Output = Result<Vec<Meta>, Error>> + Send {
            self.0.list(prefix)
        }

        fn put(
            &self,
            key: &str,
            bytes: Vec<u8>,
            _mode: PutMode,
        ) -> impl Future<Output = Result<Version, Error>> + Send {
            self.0.put(key, bytes, PutMode::Overwrite)
        }

        fn delete(&self, key: &str) -> impl Future<Output = Result<(), Error>> + Send {
            self.0.delete(key)
        }

        fn now(&self) -> u64 {
            self.0.now()
        }
    }

    /// The probe passes a bucket that honours conditional writes, and fails one that does not.
    #[test]
    fn probes_find_buckets_without_conditional_writes() {
        use crate::sim::bucket::{Direct, SimBucket};
        let sim = SimBucket::new();
        futures::executor::block_on(async {
            assert_eq!(probe(&Direct(sim.clone()), 1).await, Ok(()));
            let refused = probe(&Careless(Direct(sim.clone())), 2).await.unwrap_err();
            assert!(refused.starts_with("the store does not honour conditional writes: a create"));
        });
        assert_eq!(sim.keys(), ["probe/0000000000000002"]);
    }

    #[test]
    fn honours_conditional_writes() {
        futures::executor::block_on(probe(&*memory(), 1)).unwrap();
    }

    /// TASK-92.6: a store probed once is not probed again: the second asks nothing.
    #[test]
    fn stores_are_probed_once() {
        let store = memory();
        futures::executor::block_on(store.probed(1)).unwrap();
        let probing = store.stats();
        let writes = [Request::Create, Request::Update, Request::Delete];
        let asked = writes.map(|request| (request.name(), probing.requests(request)));
        assert_eq!(asked, [("create", 2), ("update", 2), ("delete", 1)]);
        futures::executor::block_on(store.probed(2)).unwrap();
        assert_eq!(store.stats(), probing);
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

    /// Adds a segment of `commit` alone to `writer`, closed on its chain: returns its number.
    fn add<B: Bucket>(writer: &Writer<B>, commit: Commit) -> u64 {
        writer.add_segment(Batch::from(commit).close(&mut writer.chain()).frame()).unwrap()
    }

    /// A branch's life over `bucket`: written, forked, freed, checkpointed, taken over, collected;
    /// then what it and its fork freed, reclaimed.
    pub(crate) async fn branch_lifecycle(bucket: Arc<impl Bucket>, root: u64) {
        let child = root + 1;
        branch::create(&*bucket, root).await.unwrap();
        let first = Writer::open(bucket.clone(), root, 10, 1).await.unwrap();
        let small = ExtentId::new(root, first.chain().next_sequence());
        let large = ExtentId::new(root, small.sequence + 1);
        let commit = Commit {
            tag: 1,
            extents: vec![(small, b"small".to_vec())],
            own: vec![(large, vec![7; 4096])],
            ..Commit::default()
        };
        let seq = add(&first, commit);
        first.upload(seq).await.unwrap();
        assert_eq!(first.acknowledge().await, Ok(Some(seq)));
        let forking = Batch::default().fork(child, &mut first.chain()).frame();
        let fork = first.add_segment(forking).unwrap();
        first.upload(fork).await.unwrap();
        first.acknowledge().await.unwrap();
        let seq = add(&first, Commit { tag: 2, frees: vec![small], ..Commit::default() });
        first.upload(seq).await.unwrap();
        first.acknowledge().await.unwrap();
        assert_eq!(first.checkpoint().await, Ok(Some(seq)));
        // Looked up a block at a time, by ranged reads, by a host's fetcher.
        let checkpoint = crate::layout::checkpoint(root, seq);
        let (_, footer) =
            crate::object::parts_and_footer(&*bucket, &checkpoint).await.unwrap().unwrap();
        let table = crate::checkpoint::Table::decode(&footer).unwrap();
        assert_eq!(table.state().last_commit, Some(2));
        let budget = crate::fetch::Budget::new(crate::fetch::Limits::UNBOUNDED);
        let fetcher = crate::fetch::Fetcher::new(bucket.clone(), budget, None);
        assert!(fetcher.lookup(&checkpoint, large).await.unwrap().is_some());
        assert_eq!(fetcher.lookup(&checkpoint, small).await, Ok(None));

        // Another writer takes the branch; the first one's next segment is not the branch's.
        let second = Writer::open(bucket.clone(), root, 11, 2).await.unwrap();
        let seq = add(&first, Commit { tag: 3, frees: vec![large], ..Commit::default() });
        let taken = match first.upload(seq).await {
            Ok(()) => first.acknowledge().await.map(|_| ()),
            Err(error) => Err(error),
        };
        assert_eq!(taken, Err(BranchError::Fenced));
        assert_eq!(second.state().last_commit, Some(2));

        let state = recovery::recover(&*bucket, root, None).await.unwrap().state;
        assert_eq!(state.extents.keys().copied().collect::<Vec<_>>(), [large]);
        let origin = Some(Origin { parent: root, at: fork });
        assert_eq!(branch::record(&*bucket, child).await.unwrap().unwrap().origin, origin);
        let inherited = recovery::recover(&*bucket, child, origin).await.unwrap().state;
        assert_eq!(inherited.extents.keys().copied().collect::<Vec<_>>(), [small, large]);

        // The collector keeps what the child inherited, though the parent freed it.
        let options = collector::Options { compact_below: 0.5, grace: 0 };
        collector::collect(&*bucket, 20, &options).await.unwrap();
        let location = &inherited.extents[&small];
        assert_eq!(crate::read(&bucket, small, location).await.unwrap(), b"small");
        let location = &inherited.extents[&large];
        assert_eq!(crate::read(&bucket, large, location).await.unwrap(), vec![7; 4096]);

        // Freed by the parent's writer, and by the child, which inherited them, nothing reaches
        // them: the collector reclaims their objects.
        let seq = add(&second, Commit { tag: 4, frees: vec![large], ..Commit::default() });
        second.upload(seq).await.unwrap();
        second.acknowledge().await.unwrap();
        second.checkpoint().await.unwrap();
        let heir = Writer::open(bucket.clone(), child, 12, 1).await.unwrap();
        let seq = add(&heir, Commit { tag: 1, frees: vec![small, large], ..Commit::default() });
        heir.upload(seq).await.unwrap();
        heir.acknowledge().await.unwrap();
        heir.checkpoint().await.unwrap();
        collector::collect(&*bucket, 21, &options).await.unwrap();
        for extent in [small, large] {
            let key = &inherited.extents[&extent].key;
            assert_eq!(bucket.head(key).await.map(|_| ()), Err(Error::NotFound), "{extent:?}");
        }
    }

    #[test]
    fn branches_on_a_memory_store() {
        futures::executor::block_on(branch_lifecycle(memory(), 1));
    }

    /// Probes the bucket at `url`'s conditional writes, and runs branches' lifecycle there.
    fn over_a_real_store(url: &str) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let store = Arc::new(Store::from_url(url).unwrap());
            probe(&*store, std::process::id().into()).await.unwrap();
            // Each run its own branches.
            let root = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;
            branch_lifecycle(store, root & !1).await;
        });
    }

    /// The same over a real S3 server: `scripts/test-rustfs.sh` starts RustFS and runs this with
    /// `FXDAX_S3_URL` naming a bucket of it, and the `AWS_*` variables to reach it.
    #[test]
    #[ignore]
    fn rustfs() {
        over_a_real_store(&std::env::var("FXDAX_S3_URL").expect("FXDAX_S3_URL names the bucket"));
    }

    /// The same over Azure Blob Storage, as its emulator, Azurite, serves it (TASK-46):
    /// `scripts/test-azurite.sh` starts it and runs this with `FXDAX_AZURE_URL` naming a container
    /// of it, and the `AZURE_*` variables to reach it.
    #[test]
    #[ignore]
    fn azurite() {
        let url = std::env::var("FXDAX_AZURE_URL").expect("FXDAX_AZURE_URL names the container");
        over_a_real_store(&url);
    }
}
