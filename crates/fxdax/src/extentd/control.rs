// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The daemon's control API (TASK-43.3): gRPC over a Unix socket, for the host's VM manager, which
//! launches the VMMs. Its calls and their errors are `proto/fxdax/extentd/v1/extentd.proto`'s.
//!
//! The store operations it serves are futures ([`super::stores`], TASK-43.6): their waits, for
//! uploads, guests' answers, and stores to be let go, hold no thread, and their stores' I/O runs on
//! the runtime's blocking threads, so that many calls waiting, syncs while object storage is slow,
//! keep no other call waiting.

use super::grants::Grants;
use super::memory;
use super::stores;
use crate::Errno;
use proto::extentd_client::ExtentdClient;
use proto::extentd_server::{Extentd, ExtentdServer};
use proto::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tonic::{Code, Request, Response, Status};

/// The API's messages and services, generated from its .proto.
pub mod proto {
    tonic::include_proto!("fxdax.extentd.v1");
}

/// The metadata entry of a failed call's status holding the errno it failed with, a decimal
/// number.
pub const ERRNO: &str = "fxdax-errno";

/// How many threads serve the API's calls, and their waits, which hold none (TASK-43.6).
const THREADS: usize = 2;

/// How many threads the calls' stores' I/O runs on at most: what waits holds none of them, so that
/// many calls waiting, for uploads or guests, leave them to the others' I/O.
pub const BLOCKING_THREADS: usize = 32;

/// Serves the control API to the callers that connect to `listener`, until accepting fails; the
/// sockets of the tenants it grants go in the directory `tenants`.
pub fn serve_control(
    listener: std::os::unix::net::UnixListener,
    tenants: impl Into<PathBuf>,
) -> std::io::Result<()> {
    let control = Control { grants: Arc::new(Grants::new(tenants.into())) };
    let (reporter, health) = tonic_health::server::health_reporter();
    listener.set_nonblocking(true)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(THREADS)
        .max_blocking_threads(BLOCKING_THREADS)
        .thread_name("extentd control")
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let incoming = tokio_stream::wrappers::UnixListenerStream::new(
            tokio::net::UnixListener::from_std(listener)?,
        );
        tokio::spawn(report_health(reporter));
        CONTROL_SERVING.store(true, std::sync::atomic::Ordering::Release);
        tonic::transport::Server::builder()
            .add_service(health)
            .add_service(ExtentdServer::new(control))
            .serve_with_incoming(incoming)
            .await
            .map_err(std::io::Error::other)
    })
}

/// Whether this process serves the control API ([`serve_control`]).
static CONTROL_SERVING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(super) fn is_serving() -> bool {
    CONTROL_SERVING.load(std::sync::atomic::Ordering::Acquire)
}

/// Reports, through the standard gRPC health service (grpc.health.v1.Health), whether the daemon
/// serves (TASK-48): SERVING while it accepts VM processes' connections, NOT_SERVING as it starts
/// and once it stops; the whole server's status, named "", and the control API's, alike.
async fn report_health(reporter: tonic_health::server::HealthReporter) {
    use tonic::server::NamedService;
    use tonic_health::ServingStatus;
    let mut serving = super::SERVING.subscribe();
    loop {
        let now = *serving.borrow_and_update();
        let status = if now { ServingStatus::Serving } else { ServingStatus::NotServing };
        reporter.set_service_status("", status).await;
        reporter.set_service_status(<ExtentdServer<Control> as NamedService>::NAME, status).await;
        super::REPORTED.send_replace(now);
        if serving.changed().await.is_err() {
            return;
        }
    }
}

/// A channel to the control API of the daemon listening at `socket`: for its clients, the API's
/// and its health service's.
pub async fn channel(
    socket: impl AsRef<Path>,
) -> Result<tonic::transport::Channel, tonic::transport::Error> {
    let socket = socket.as_ref().to_owned();
    // The URI names no host: the connector connects to the socket, whatever it asks for.
    tonic::transport::Endpoint::from_static("http://extentd")
        .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
            let socket = socket.clone();
            async move {
                let stream = tokio::net::UnixStream::connect(socket).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
            }
        }))
        .await
}

/// A client of the control API of the daemon listening at `socket`.
pub async fn connect(
    socket: impl AsRef<Path>,
) -> Result<ExtentdClient<tonic::transport::Channel>, tonic::transport::Error> {
    Ok(ExtentdClient::new(channel(socket).await?))
}

/// The errno a call failed with, as its status's [`ERRNO`] entry has it, if it has one.
pub fn errno(status: &Status) -> Option<Errno> {
    let errno = status.metadata().get(ERRNO)?.to_str().ok()?;
    errno.parse().ok().map(Errno::from_raw)
}

/// The status of a call that failed with `errno`.
fn status(errno: Errno) -> Status {
    let code = match errno {
        Errno::ENOENT => Code::NotFound,
        Errno::EEXIST => Code::AlreadyExists,
        Errno::EACCES | Errno::EPERM => Code::PermissionDenied,
        Errno::EINVAL => Code::InvalidArgument,
        Errno::ENOSPC => Code::ResourceExhausted,
        Errno::ETIMEDOUT => Code::DeadlineExceeded,
        Errno::EBUSY | Errno::ESTALE | Errno::ENOTSUP => Code::FailedPrecondition,
        _ => Code::Internal,
    };
    let mut status = Status::new(code, format!("{errno:?}"));
    status.metadata_mut().insert(ERRNO, errno.raw().into());
    status
}

/// The reply to a call that did, or failed with an errno.
/// What StatStore says of a store that is and holds `state`.
fn stat_response(state: stores::StoreState) -> StatStoreResponse {
    let upload = state.upload.unwrap_or(crate::remote::UploadStatus {
        lease: None,
        uploaded: 0,
        pending_bytes: 0,
        oldest_pending: None,
    });
    let millis = |time: std::time::SystemTime| {
        time.duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64)
    };
    let waited = |since: std::time::SystemTime| {
        since.elapsed().map_or(0, |waited| waited.as_millis() as u64)
    };
    StatStoreResponse {
        branch: state.branch,
        capacity: state.info.capacity,
        used: state.info.used,
        available: state.info.available,
        public: state.public,
        open: state.open,
        own: state.bytes.own,
        inherited: state.bytes.inherited,
        committed: state.committed,
        uploading: state.upload.is_some(),
        lease_token: upload.lease.map_or(0, |lease| lease.token),
        lease_epoch: upload.lease.map_or(0, |lease| lease.epoch),
        lease_taken_unix_ms: upload.lease.map_or(0, |lease| millis(lease.taken)),
        uploaded: upload.uploaded,
        pending_upload_bytes: upload.pending_bytes,
        oldest_pending_ms: upload.oldest_pending.map_or(0, waited),
    }
}

fn reply<T>(done: Result<T, Errno>) -> Result<Response<T>, Status> {
    done.map(Response::new).map_err(status)
}

/// The store a request names by `path`.
fn path(path: String) -> Result<PathBuf, Status> {
    if path.is_empty() {
        return Err(status(Errno::EINVAL));
    }
    Ok(PathBuf::from(path))
}

/// The API's calls, served from the daemon's stores, and its grants.
struct Control {
    grants: Arc<Grants>,
}

#[tonic::async_trait]
impl Extentd for Control {
    async fn create_store(
        &self,
        request: Request<CreateStoreRequest>,
    ) -> Result<Response<CreateStoreResponse>, Status> {
        let CreateStoreRequest { path: store, capacity, branch, token } = request.into_inner();
        let store = path(store)?;
        let branch = stores::create(store, capacity, Some(branch), token).await;
        reply(branch.map(|branch| CreateStoreResponse { branch }))
    }

    async fn import_image(
        &self,
        request: Request<ImportImageRequest>,
    ) -> Result<Response<ImportImageResponse>, Status> {
        let ImportImageRequest { path: store, capacity, image, reference } = request.into_inner();
        let (store, image) = (path(store)?, path(image)?);
        let reference = Some(reference).filter(|reference| !reference.is_empty());
        let branch = stores::import(store, capacity, image, reference).await;
        reply(branch.map(|branch| ImportImageResponse { branch }))
    }

    async fn fork_store(
        &self,
        request: Request<ForkStoreRequest>,
    ) -> Result<Response<ForkStoreResponse>, Status> {
        let ForkStoreRequest { from, to, branch } = request.into_inner();
        let (from, to) = (path(from)?, path(to)?);
        let branch = if branch == 0 { rand::random::<u64>().max(1) } else { branch };
        reply(stores::fork(from, to, branch).await.map(|()| ForkStoreResponse { branch }))
    }

    async fn delete_store(
        &self,
        request: Request<DeleteStoreRequest>,
    ) -> Result<Response<DeleteStoreResponse>, Status> {
        let store = path(request.into_inner().path)?;
        reply(stores::delete(store).await.map(|()| DeleteStoreResponse {}))
    }

    async fn resize_store(
        &self,
        request: Request<ResizeStoreRequest>,
    ) -> Result<Response<ResizeStoreResponse>, Status> {
        let ResizeStoreRequest { path: store, capacity } = request.into_inner();
        let store = path(store)?;
        reply(stores::resize(store, capacity).await.map(|()| ResizeStoreResponse {}))
    }

    async fn place_store(
        &self,
        request: Request<PlaceStoreRequest>,
    ) -> Result<Response<PlaceStoreResponse>, Status> {
        let PlaceStoreRequest { path: store, token } = request.into_inner();
        let store = path(store)?;
        reply(stores::place(store, token).await.map(|()| PlaceStoreResponse {}))
    }

    async fn make_public(
        &self,
        request: Request<MakePublicRequest>,
    ) -> Result<Response<MakePublicResponse>, Status> {
        let store = path(request.into_inner().path)?;
        reply(stores::make_public(store).await.map(|()| MakePublicResponse {}))
    }

    async fn sync_store(
        &self,
        request: Request<SyncStoreRequest>,
    ) -> Result<Response<SyncStoreResponse>, Status> {
        let store = path(request.into_inner().path)?;
        reply(stores::sync(store).await.map(|()| SyncStoreResponse {}))
    }

    async fn stat_store(
        &self,
        request: Request<StatStoreRequest>,
    ) -> Result<Response<StatStoreResponse>, Status> {
        let store = path(request.into_inner().path)?;
        reply(stores::stat(store).await.map(stat_response))
    }

    async fn list_stores(
        &self,
        _request: Request<ListStoresRequest>,
    ) -> Result<Response<ListStoresResponse>, Status> {
        let listed = stores::list().map(|stores| {
            let stores = stores.into_iter().map(|(path, state)| ListedStore {
                path: path.to_string_lossy().into_owned(),
                stat: Some(stat_response(state)),
            });
            ListStoresResponse { stores: stores.collect() }
        });
        reply(listed)
    }

    async fn get_stats(
        &self,
        _request: Request<GetStatsRequest>,
    ) -> Result<Response<GetStatsResponse>, Status> {
        let crate::stats::Stats {
            maps,
            map_ns,
            faults,
            fault_ns,
            page_ins,
            page_in_bytes,
            fetched_bytes,
            reads,
            read_bytes,
            read_stored_bytes,
            prefetches,
            unpacks,
            // The VMM processes', which change their windows, not the daemon.
            stage_ops: _,
            stage_ns: _,
            commits,
            commit_ns,
            commit_write_ns,
            commit_written_bytes,
            commit_store_ns,
            commit_seal_ns,
            commit_wait_ns,
        } = crate::stats::stats();
        use extent_remote::store::{Request, Upload};
        let bucket = stores::remote_bucket().map_err(status)?.stats();
        let object_storage = Some(ObjectStorageStats {
            requests: (Request::ALL.into_iter())
                .map(|request| (request.name().to_owned(), bucket.requests(request)))
                .collect(),
            uploaded_bytes: (Upload::ALL.into_iter())
                .map(|upload| (upload.name().to_owned(), bucket.uploaded(upload)))
                .collect(),
            downloaded_bytes: bucket.downloaded,
        });
        Ok(Response::new(GetStatsResponse {
            maps,
            map_ns,
            faults,
            fault_ns,
            page_ins,
            page_in_bytes,
            fetched_bytes,
            reads,
            read_bytes,
            read_stored_bytes,
            prefetches,
            unpacks,
            object_storage,
            commits,
            commit_ns,
            commit_write_ns,
            commit_written_bytes,
            commit_store_ns,
            commit_seal_ns,
            commit_wait_ns,
        }))
    }

    async fn grant(
        &self,
        request: Request<GrantRequest>,
    ) -> Result<Response<GrantResponse>, Status> {
        let GrantRequest { tenant, dir, uid, gid } = request.into_inner();
        let dir = path(dir)?;
        let grants = self.grants.clone();
        let granted = stores::blocking(move || {
            let socket = grants.grant(&tenant, &dir, uid, gid)?;
            let socket = socket.into_os_string().into_string().map_err(|_| Errno::EINVAL)?;
            Ok(GrantResponse { socket })
        });
        reply(granted.await)
    }

    async fn revoke(
        &self,
        request: Request<RevokeRequest>,
    ) -> Result<Response<RevokeResponse>, Status> {
        let tenant = request.into_inner().tenant;
        let grants = self.grants.clone();
        reply(stores::blocking(move || grants.revoke(&tenant).map(|()| RevokeResponse {})).await)
    }

    async fn set_memory_limit(
        &self,
        request: Request<SetMemoryLimitRequest>,
    ) -> Result<Response<SetMemoryLimitResponse>, Status> {
        let SetMemoryLimitRequest { tenant, bytes } = request.into_inner();
        let tenant = Some(tenant).filter(|tenant| !tenant.is_empty());
        reply(memory::set_limit(tenant.as_deref(), bytes).map(|()| SetMemoryLimitResponse {}))
    }

    async fn shrink_memory(
        &self,
        request: Request<ShrinkMemoryRequest>,
    ) -> Result<Response<ShrinkMemoryResponse>, Status> {
        let ShrinkMemoryRequest { tenant, target_bytes } = request.into_inner();
        let tenant = Some(tenant).filter(|tenant| !tenant.is_empty());
        let shrunk = stores::blocking(move || memory::shrink(tenant.as_deref(), target_bytes));
        reply(shrunk.await.map(|bytes| ShrinkMemoryResponse { bytes }))
    }
}

/// A client of a daemon's control API that blocks, for callers without a runtime of their own:
/// fxdaxctl, and tests. Its errors carry the errno a call failed with, if it did
/// ([`crate::Error::errno`]).
pub struct Client {
    runtime: tokio::runtime::Runtime,
    client: ExtentdClient<tonic::transport::Channel>,
}

impl Client {
    /// A client of the daemon whose control socket is at `socket`.
    pub fn connect(socket: impl AsRef<Path>) -> Result<Self, crate::Error> {
        let socket = socket.as_ref();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(crate::Error::new)?;
        let client = runtime.block_on(connect(socket)).map_err(|err| {
            crate::Error::new(anyhow::Error::new(err).context(format!(
                "connecting to the extent daemon's control socket at {}",
                socket.display()
            )))
        })?;
        Ok(Self { runtime, client })
    }

    /// Makes an empty store at `path` of `capacity` bytes, of a new branch or `branch`, placed at
    /// `token` if given, and returns its branch (`CreateStore`).
    pub fn create_store(
        &self,
        path: &Path,
        capacity: u64,
        branch: Option<u64>,
        token: Option<u64>,
    ) -> Result<u64, crate::Error> {
        let request =
            CreateStoreRequest { path: name(path)?, capacity, branch: branch.unwrap_or(0), token };
        let response = self
            .call(format!("making the store at {}", path.display()), |mut c| async move {
                c.create_store(request).await
            })?;
        Ok(response.branch)
    }

    /// Makes a store at `path` of `capacity` bytes holding the container image at `image`, the one
    /// `reference` names in it if given, and returns its branch (`ImportImage`).
    pub fn import_image(
        &self,
        path: &Path,
        capacity: u64,
        image: &Path,
        reference: Option<&str>,
    ) -> Result<u64, crate::Error> {
        let request = ImportImageRequest {
            path: name(path)?,
            capacity,
            image: name(image)?,
            reference: reference.unwrap_or_default().to_owned(),
        };
        let response = self.call(format!("importing {}", image.display()), |mut c| async move {
            c.import_image(request).await
        })?;
        Ok(response.branch)
    }

    /// Forks the store at `from` to a new branch at `to`, and returns the branch (`ForkStore`).
    pub fn fork_store(&self, from: &Path, to: &Path) -> Result<u64, crate::Error> {
        let request = ForkStoreRequest { from: name(from)?, to: name(to)?, branch: 0 };
        let what = format!("forking the store at {} to {}", from.display(), to.display());
        let response = self.call(what, |mut c| async move { c.fork_store(request).await })?;
        Ok(response.branch)
    }

    /// Deletes the store at `path` (`DeleteStore`).
    pub fn delete_store(&self, path: &Path) -> Result<(), crate::Error> {
        let request = DeleteStoreRequest { path: name(path)? };
        let what = format!("deleting the store at {}", path.display());
        self.call(what, |mut c| async move { c.delete_store(request).await }).map(drop)
    }

    /// Sets the capacity of the store at `path` to `capacity` bytes (`ResizeStore`).
    pub fn resize_store(&self, path: &Path, capacity: u64) -> Result<(), crate::Error> {
        let request = ResizeStoreRequest { path: name(path)?, capacity };
        let what = format!("resizing the store at {} to {capacity} bytes", path.display());
        self.call(what, |mut c| async move { c.resize_store(request).await }).map(drop)
    }

    /// Places the store at `path` at the placement whose fencing token is `token` (`PlaceStore`).
    pub fn place_store(&self, path: &Path, token: u64) -> Result<(), crate::Error> {
        let request = PlaceStoreRequest { path: name(path)?, token };
        let what = format!("placing the store at {}", path.display());
        self.call(what, |mut c| async move { c.place_store(request).await }).map(drop)
    }

    /// Makes the store at `path` a public base (`MakePublic`).
    pub fn make_public(&self, path: &Path) -> Result<(), crate::Error> {
        let request = MakePublicRequest { path: name(path)? };
        let what = format!("making the store at {} public", path.display());
        self.call(what, |mut c| async move { c.make_public(request).await }).map(drop)
    }

    /// What the store at `path` is, and holds (`StatStore`).
    pub fn stat_store(&self, path: &Path) -> Result<StatStoreResponse, crate::Error> {
        let request = StatStoreRequest { path: name(path)? };
        let what = format!("asking what the store at {} holds", path.display());
        self.call(what, |mut c| async move { c.stat_store(request).await })
    }

    /// The stores the daemon serves, with what each is and holds (`ListStores`).
    pub fn list_stores(&self) -> Result<Vec<ListedStore>, crate::Error> {
        let what = "listing the daemon's stores".to_owned();
        let listed =
            self.call(what, |mut c| async move { c.list_stores(ListStoresRequest {}).await })?;
        Ok(listed.stores)
    }

    /// The epoch of the latest commit of the store at `path` (`StatStore`'s `committed`).
    pub fn committed(&self, path: &Path) -> Result<u64, crate::Error> {
        Ok(self.stat_store(path)?.committed)
    }

    /// Waits until every commit of the store at `path` is in the daemon's bucket (`SyncStore`).
    pub fn sync_store(&self, path: &Path) -> Result<(), crate::Error> {
        let request = SyncStoreRequest { path: name(path)? };
        let what = format!("waiting for the store at {} to be uploaded", path.display());
        self.call(what, |mut c| async move { c.sync_store(request).await }).map(drop)
    }

    /// Limits the memory of the tenant `tenant`, or of every tenant without one, to `bytes`, or
    /// lifts the limit without them (`SetMemoryLimit`).
    pub fn set_memory_limit(
        &self,
        tenant: Option<&str>,
        bytes: Option<u64>,
    ) -> Result<(), crate::Error> {
        let request =
            SetMemoryLimitRequest { tenant: tenant.unwrap_or_default().to_owned(), bytes };
        let what = format!("limiting the memory of {}", tenant.unwrap_or("every tenant"));
        self.call(what, |mut c| async move { c.set_memory_limit(request).await }).map(drop)
    }

    /// Evicts pages until the tenant `tenant`, or the daemon without one, holds at most `target`
    /// bytes, or none is left to evict, and returns what it holds then (`ShrinkMemory`).
    pub fn shrink_memory(&self, tenant: Option<&str>, target: u64) -> Result<u64, crate::Error> {
        let request = ShrinkMemoryRequest {
            tenant: tenant.unwrap_or_default().to_owned(),
            target_bytes: target,
        };
        let what = format!("shrinking the memory of {}", tenant.unwrap_or("the daemon"));
        let response = self.call(what, |mut c| async move { c.shrink_memory(request).await })?;
        Ok(response.bytes)
    }

    /// The daemon's counters (`GetStats`).
    pub fn stats(&self) -> Result<crate::stats::Stats, crate::Error> {
        let response = self
            .call("asking the extent daemon for its stats".to_owned(), |mut c| async move {
                c.get_stats(GetStatsRequest {}).await
            })?;
        Ok(crate::stats::Stats {
            maps: response.maps,
            map_ns: response.map_ns,
            faults: response.faults,
            fault_ns: response.fault_ns,
            page_ins: response.page_ins,
            page_in_bytes: response.page_in_bytes,
            fetched_bytes: response.fetched_bytes,
            reads: response.reads,
            read_bytes: response.read_bytes,
            read_stored_bytes: response.read_stored_bytes,
            prefetches: response.prefetches,
            unpacks: response.unpacks,
            // The VMM processes' (`Stats::stage_ops`).
            stage_ops: 0,
            stage_ns: 0,
            commits: response.commits,
            commit_ns: response.commit_ns,
            commit_write_ns: response.commit_write_ns,
            commit_written_bytes: response.commit_written_bytes,
            commit_store_ns: response.commit_store_ns,
            commit_seal_ns: response.commit_seal_ns,
            commit_wait_ns: response.commit_wait_ns,
        })
    }

    /// Makes the call `call` makes with a client, and returns its response; or, failed, an error
    /// saying it was `what`, with the errno it failed with, if any.
    fn call<T, F>(
        &self,
        what: String,
        call: impl FnOnce(ExtentdClient<tonic::transport::Channel>) -> F,
    ) -> Result<T, crate::Error>
    where
        F: Future<Output = Result<Response<T>, Status>>,
    {
        match self.runtime.block_on(call(self.client.clone())) {
            Ok(response) => Ok(response.into_inner()),
            Err(status) => Err(crate::Error::new(match errno(&status) {
                Some(errno) => anyhow::Error::new(errno).context(what),
                None => anyhow::Error::new(status).context(what),
            })),
        }
    }
}

/// `path`, as a request names it: `EINVAL` unless it is UTF-8.
fn name(path: &Path) -> Result<String, crate::Error> {
    path.to_str().map(str::to_owned).ok_or_else(|| crate::Error::from(Errno::EINVAL))
}
