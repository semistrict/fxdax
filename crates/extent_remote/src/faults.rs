// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a bucket as a degraded one is, between a store and its objects ([`Faulty`]): each
//! request waits so long first, and fails as it is told, its writes as a full disk's do or every
//! request as an object store out of service answers, until it is told otherwise (TASK-330.3).
//! What a host's daemon does meanwhile, its memory, its stalled commits and what it says of the
//! failures, is what a soak against it measures; how far a bucket is, its latency, what LangSmith's
//! RustFS beside the daemon on a shared disk was (TASK-310), and what `FXDAX_REMOTE_LATENCY_MS`
//! set alone before.
//!
//! A bucket made from a URL ([`crate::store::object_store_at`]) is one, if its process's
//! environment says so: `FXDAX_REMOTE_LATENCY_MS`, milliseconds each request waits, as long as the
//! process lives (TASK-62); `FXDAX_REMOTE_FAULTS`, a file of what it does now, read again within
//! [`READ_EVERY`] of each change, for a test to change as it runs: [`Fault::parse`]'s words, such
//! as `latency-ms=300 failing=no-space`, none for a bucket that answers. Neither set, a bucket is
//! the object store itself: nothing of this is in its requests' way.

use futures::StreamExt as _;
use futures::stream::BoxStream;
use object_store::path::Path;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    PutMultipartOptions, PutOptions, PutPayload, PutResult,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often a file of faults is read again, at most, as requests come: what a phase's change
/// waits to take effect.
pub const READ_EVERY: Duration = Duration::from_millis(100);

/// How a bucket fails its requests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Failing {
    /// It doesn't.
    #[default]
    Not,
    /// Its writes fail as a file bucket's on a full disk do, `object_store`'s generic error with
    /// the filesystem's `ENOSPC` behind it (TASK-314); its reads and deletes answer.
    NoSpace,
    /// Every request fails as an object store out of service answers them, `503 Service
    /// Unavailable` once `object_store`'s own retries gave up.
    Unavailable,
}

impl Failing {
    const NAMES: [(Self, &'static str); 3] =
        [(Self::Not, "not"), (Self::NoSpace, "no-space"), (Self::Unavailable, "unavailable")];

    /// Its name in a [`Fault`]'s words.
    pub fn name(self) -> &'static str {
        Self::NAMES.iter().find(|(failing, _)| *failing == self).unwrap().1
    }
}

/// What a degraded bucket does to each request: waits `latency` first, then fails it as `failing`
/// says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fault {
    pub latency: Duration,
    pub failing: Failing,
}

impl Fault {
    /// The fault `words` say, words separated by white space, each `NAME=VALUE`:
    /// `latency-ms=MILLISECONDS` and `failing=not|no-space|unavailable`, each of the default fault's
    /// (none) unless said.
    pub fn parse(words: &str) -> Result<Self, String> {
        let mut fault = Self::default();
        for word in words.split_whitespace() {
            match word.split_once('=') {
                Some(("latency-ms", ms)) => {
                    let ms = ms.parse().map_err(|_| format!("latency-ms={ms} is no count"))?;
                    fault.latency = Duration::from_millis(ms);
                }
                Some(("failing", name)) => {
                    fault.failing = (Failing::NAMES.iter())
                        .find(|(_, known)| *known == name)
                        .map(|(failing, _)| *failing)
                        .ok_or_else(|| format!("failing={name}: not, no-space or unavailable"))?;
                }
                _ => return Err(format!("{word}: latency-ms=MILLISECONDS or failing=HOW")),
            }
        }
        Ok(fault)
    }

    /// Its words, which [`Fault::parse`] reads.
    pub fn words(&self) -> String {
        format!("latency-ms={} failing={}", self.latency.as_millis(), self.failing.name())
    }

    /// What it fails a request with, a write if `write`, if it fails it.
    fn failure(&self, write: bool) -> object_store::Result<()> {
        let source: Box<dyn std::error::Error + Send + Sync> = match self.failing {
            Failing::NoSpace if write => Box::new(std::io::Error::from_raw_os_error(libc::ENOSPC)),
            Failing::Unavailable => "HTTP status server error (503 Service Unavailable)".into(),
            Failing::Not | Failing::NoSpace => return Ok(()),
        };
        Err(object_store::Error::Generic { store: "Faulty", source })
    }
}

/// What a bucket's faults are now: a fault set, and what a file says, as of when it was last read.
#[derive(Debug)]
pub struct Faults {
    /// What every request waits besides, as long as these live (`FXDAX_REMOTE_LATENCY_MS`).
    latency: Duration,
    /// The file it reads its fault from, if any (`FXDAX_REMOTE_FAULTS`).
    file: Option<std::path::PathBuf>,
    now: Mutex<Now>,
}

#[derive(Debug)]
struct Now {
    fault: Fault,
    /// When the file was last read.
    read: Option<Instant>,
}

impl Faults {
    /// Faults whose every request waits `latency` besides, of `file`'s, if any, read as requests
    /// come, or of what [`Faults::set`] sets.
    pub fn new(latency: Duration, file: Option<std::path::PathBuf>) -> Arc<Self> {
        let now = Mutex::new(Now { fault: Fault::default(), read: None });
        Arc::new(Self { latency, file, now })
    }

    /// The faults the process's environment says ([`self`]), if it says any.
    pub(crate) fn from_env() -> Result<Option<Arc<Self>>, String> {
        let latency = match std::env::var("FXDAX_REMOTE_LATENCY_MS") {
            Ok(ms) if !ms.is_empty() => Duration::from_millis(
                ms.parse().map_err(|_| format!("FXDAX_REMOTE_LATENCY_MS={ms} is no count"))?,
            ),
            _ => Duration::ZERO,
        };
        let file = std::env::var_os("FXDAX_REMOTE_FAULTS").filter(|file| !file.is_empty());
        if latency.is_zero() && file.is_none() {
            return Ok(None);
        }
        let faults = Self::new(latency, file.map(Into::into));
        // A file that says no fault it can read fails now, not at a request.
        faults.read()?;
        Ok(Some(faults))
    }

    /// Has the requests from now on do as `fault` says, until a file's says otherwise.
    pub fn set(&self, fault: Fault) {
        self.now.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).fault = fault;
    }

    /// What a request does now: its file's, read again if it was last read [`READ_EVERY`] ago, or
    /// what was set, waiting besides what every request waits.
    pub fn now(&self) -> Fault {
        let due = {
            let now = self.now.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            self.file.is_some() && now.read.is_none_or(|read| read.elapsed() >= READ_EVERY)
        };
        if due && let Err(error) = self.read() {
            log::warn!("object storage: its faults: {error}");
        }
        let fault = self.now.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).fault;
        Fault { latency: fault.latency + self.latency, ..fault }
    }

    /// Reads its file, if it has one: a file gone, or empty, says no fault.
    fn read(&self) -> Result<(), String> {
        let Some(file) = &self.file else { return Ok(()) };
        let words = match std::fs::read_to_string(file) {
            Ok(words) => words,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(format!("reading {}: {error}", file.display())),
        };
        let fault = Fault::parse(&words).map_err(|error| format!("{}: {error}", file.display()));
        let mut now = self.now.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        now.read = Some(Instant::now());
        let fault = fault?;
        if fault != now.fault {
            log::info!("object storage: as {} says, {}", file.display(), fault.words());
            now.fault = fault;
        }
        Ok(())
    }
}

/// An object store as degraded as its [`Faults`] say, before `objects`.
#[derive(Debug)]
pub struct Faulty {
    objects: Arc<dyn ObjectStore>,
    faults: Arc<Faults>,
}

impl Faulty {
    pub fn new(objects: Arc<dyn ObjectStore>, faults: Arc<Faults>) -> Self {
        Self { objects, faults }
    }

    /// Waits as a request, a write if `write`, does now, then fails it, or doesn't.
    async fn request(&self, write: bool) -> object_store::Result<()> {
        let fault = self.faults.now();
        if !fault.latency.is_zero() {
            tokio::time::sleep(fault.latency).await;
        }
        fault.failure(write)
    }

    /// A stream of what `stream` yields, once a request, not a write, has waited and failed, or
    /// not.
    fn streamed<T: Send + 'static>(
        &self,
        stream: BoxStream<'static, object_store::Result<T>>,
    ) -> BoxStream<'static, object_store::Result<T>> {
        let fault = self.faults.now();
        let waited = async move {
            if !fault.latency.is_zero() {
                tokio::time::sleep(fault.latency).await;
            }
            match fault.failure(false) {
                Ok(()) => stream,
                Err(error) => futures::stream::once(async { Err(error) }).boxed(),
            }
        };
        futures::stream::once(waited).flatten().boxed()
    }
}

impl std::fmt::Display for Faulty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Faulty({})", self.objects)
    }
}

#[async_trait::async_trait]
impl ObjectStore for Faulty {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        self.request(true).await?;
        self.objects.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.request(true).await?;
        self.objects.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &Path,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        self.request(false).await?;
        self.objects.get_opts(location, options).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<Path>>,
    ) -> BoxStream<'static, object_store::Result<Path>> {
        self.streamed(self.objects.delete_stream(locations))
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        self.streamed(self.objects.list(prefix))
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> object_store::Result<ListResult> {
        self.request(false).await?;
        self.objects.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &Path,
        to: &Path,
        options: CopyOptions,
    ) -> object_store::Result<()> {
        self.request(true).await?;
        self.objects.copy_opts(from, to, options).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fault's words say it, and what it says them as reads back as it.
    #[test]
    fn faults_say_their_words() {
        let fault = Fault::parse("latency-ms=300 failing=no-space").unwrap();
        assert_eq!(fault, Fault { latency: Duration::from_millis(300), failing: Failing::NoSpace });
        assert_eq!(fault.words(), "latency-ms=300 failing=no-space");
        assert_eq!(Fault::parse(&fault.words()), Ok(fault));
        assert_eq!(Fault::parse(" \n"), Ok(Fault::default()));
        assert_eq!(
            Fault::parse("failing=unavailable"),
            Ok(Fault { latency: Duration::ZERO, failing: Failing::Unavailable })
        );
        assert_eq!(
            Fault::parse("failing=sometimes"),
            Err("failing=sometimes: not, no-space or unavailable".to_owned())
        );
        assert_eq!(
            Fault::parse("latency=3"),
            Err("latency=3: latency-ms=MILLISECONDS or failing=HOW".to_owned())
        );
    }

    /// A bucket whose disk is full fails its writes with the filesystem's ENOSPC, and answers its
    /// reads; one out of service fails every request; each request waits its latency first; and
    /// its file's faults hold within [`READ_EVERY`] of its change, none once it is gone.
    #[tokio::test]
    async fn requests_fail_as_their_faults_say() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("faults");
        let faults = Faults::new(Duration::ZERO, Some(file.clone()));
        let objects = Arc::new(object_store::memory::InMemory::new());
        let store = Faulty::new(objects, faults.clone());
        let path = Path::from("a");
        let put = || store.put_opts(&path, PutPayload::from_static(b"1"), PutOptions::default());
        put().await.unwrap();

        std::fs::write(&file, "failing=no-space").unwrap();
        tokio::time::sleep(READ_EVERY).await;
        let error = put().await.unwrap_err().to_string();
        assert_eq!(error, "Generic Faulty error: No space left on device (os error 28)");
        assert_eq!(store.get_opts(&path, GetOptions::default()).await.unwrap().meta.size, 1);

        std::fs::write(&file, "latency-ms=200 failing=unavailable").unwrap();
        tokio::time::sleep(READ_EVERY).await;
        let started = Instant::now();
        let error = store.get_opts(&path, GetOptions::default()).await.unwrap_err();
        assert!(started.elapsed() >= Duration::from_millis(200), "{:?}", started.elapsed());
        assert_eq!(
            error.to_string(),
            "Generic Faulty error: HTTP status server error (503 Service Unavailable)"
        );
        let listed: Vec<_> = store.list(None).collect().await;
        assert_eq!(listed.len(), 1);
        assert!(listed[0].is_err());
        // fxdax: listings by delimiter and batches of deletes fail as every request (TASK-395.5).
        let unavailable =
            "Generic Faulty error: HTTP status server error (503 Service Unavailable)";
        let level = store.list_with_delimiter(None).await.unwrap_err();
        assert_eq!(level.to_string(), unavailable);
        let deleting = futures::stream::iter([Ok(path.clone()), Ok(Path::from("b"))]).boxed();
        let deleted: Vec<String> = (store.delete_stream(deleting).collect::<Vec<_>>().await)
            .into_iter()
            .map(|deleted| deleted.unwrap_err().to_string())
            .collect();
        assert_eq!(deleted, [unavailable]);

        std::fs::remove_file(&file).unwrap();
        tokio::time::sleep(READ_EVERY).await;
        assert_eq!(faults.now(), Fault::default());
        put().await.unwrap();
        let listed: Vec<_> = store.list(None).collect().await;
        assert_eq!(
            listed.into_iter().map(|meta| meta.unwrap().location).collect::<Vec<_>>(),
            std::slice::from_ref(&path)
        );
        let level = store.list_with_delimiter(None).await.unwrap();
        let objects: Vec<Path> = level.objects.into_iter().map(|meta| meta.location).collect();
        assert_eq!((level.common_prefixes, objects), (vec![], vec![path.clone()]));
        let deleting = futures::stream::iter([Ok(path.clone())]).boxed();
        let deleted: Vec<Path> =
            store.delete_stream(deleting).map(|deleted| deleted.unwrap()).collect().await;
        assert_eq!(deleted, [path]);
        assert_eq!(store.list(None).collect::<Vec<_>>().await.len(), 0);
    }
}
