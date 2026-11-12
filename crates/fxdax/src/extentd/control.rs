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

use super::disk;
use super::grants::Grants;
use super::memory;
use super::stores;
use crate::Errno;
use extent_remote::cell::Kind;
use proto::extentd_client::ExtentdClient;
use proto::extentd_server::{Extentd, ExtentdServer};
use proto::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tonic::codegen::http;
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
        // Its first status reported before it serves: the reporter starts with the server's, "",
        // SERVING, which a check served before a task of its own reported otherwise saw.
        let mut serving = super::SERVING.subscribe();
        let now = *serving.borrow_and_update();
        report_health(&reporter, now).await;
        tokio::spawn(async move {
            while serving.changed().await.is_ok() {
                let now = *serving.borrow_and_update();
                report_health(&reporter, now).await;
            }
        });
        CONTROL_SERVING.store(true, std::sync::atomic::Ordering::Release);
        tonic::transport::Server::builder()
            .add_service(health)
            .add_service(Drained(ExtentdServer::new(control)))
            .serve_with_incoming(incoming)
            .await
            .map_err(std::io::Error::other)
    })
}

/// fxdax: `snapshot`, as the API has it (TASK-111.4).
fn snapshot_response(snapshot: super::snapshot::Snapshot) -> GetSnapshotResponse {
    let named = |pairs: &[(&str, u64)]| -> std::collections::HashMap<String, u64> {
        pairs.iter().map(|(name, value)| ((*name).to_owned(), *value)).collect()
    };
    let vms = snapshot.vms.into_iter().map(|vm| VmSnapshot {
        connection: vm.connection,
        branch: vm.branch,
        path: vm.path.to_string_lossy().into_owned(),
        follower: vm.follower,
        build: vm.build.clone().unwrap_or_default(),
        counters: named(&vm.counters()),
        gauges: named(&vm.gauges()),
        window: vm.window().map_or_else(Default::default, |window| named(&window)),
        answered: vm.answered.is_some(),
        answer_ns: vm.answered.map_or(0, |took| took.as_nanos() as u64),
        guest: vm.guest.map(|guest| GuestSnapshot {
            build: guest.build.unwrap_or_default(),
            counters: guest.counters.into_iter().collect(),
            histograms: (guest.histograms.into_iter())
                .map(|(name, buckets)| (name, Histogram { buckets }))
                .collect(),
        }),
    });
    let taken = snapshot.taken.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    GetSnapshotResponse {
        taken_unix_ns: taken.as_nanos() as u64,
        build: snapshot.build.to_owned(),
        counters: snapshot.counters.into_iter().collect(),
        gauges: named(&snapshot.gauges),
        vms: vms.collect(),
    }
}

/// fxdax: the API's calls in flight, which a drain waits for, refusing more (TASK-141.4).
pub(super) static CALLS: super::drain::Work = super::drain::Work::new();

/// fxdax: the API's service, `S`, its calls counted in [`CALLS`] until they are answered, and
/// refused (`UNAVAILABLE`) as the daemon drains (TASK-141.4): what a call does is done once it is
/// answered, as none streams. The health service's calls are not, as its watches never end.
#[derive(Clone)]
struct Drained<S>(S);

impl<S: tonic::server::NamedService> tonic::server::NamedService for Drained<S> {
    const NAME: &'static str = S::NAME;
}

impl<S, B, Answer> tower::Service<http::Request<B>> for Drained<S>
where
    S: tower::Service<http::Request<B>, Response = http::Response<Answer>>,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    Answer: Default + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = futures::future::BoxFuture<'static, Result<S::Response, S::Error>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        let Some(call) = CALLS.begin() else {
            let refused = Status::unavailable("the extent daemon drains, as it is about to exit");
            return Box::pin(std::future::ready(Ok(refused.into_http())));
        };
        let answered = self.0.call(request);
        Box::pin(async move {
            let answered = answered.await;
            drop(call);
            answered
        })
    }
}

/// Whether this process serves the control API ([`serve_control`]).
static CONTROL_SERVING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(super) fn is_serving() -> bool {
    CONTROL_SERVING.load(std::sync::atomic::Ordering::Acquire)
}

/// Reports, through the standard gRPC health service (grpc.health.v1.Health), whether the daemon
/// serves (TASK-48), `serving`: SERVING while it accepts VM processes' connections, NOT_SERVING
/// as it starts and once it stops; the whole server's status, named "", and the control API's,
/// alike. Its callers have it reported once it returns ([`super::stop_serving`] waits for that).
async fn report_health(reporter: &tonic_health::server::HealthReporter, serving: bool) {
    use tonic::server::NamedService;
    use tonic_health::ServingStatus;
    let status = if serving { ServingStatus::Serving } else { ServingStatus::NotServing };
    reporter.set_service_status("", status).await;
    reporter.set_service_status(<ExtentdServer<Control> as NamedService>::NAME, status).await;
    super::REPORTED.send_replace(serving);
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
    saying(errno, format!("{errno:?}"))
}

/// The status of a call that failed with `errno`, saying `message`: as this API's errors are, and
/// those of the API of FUSE mounts of many stores (`crate::fuse::control`).
pub(crate) fn saying(errno: Errno, message: String) -> Status {
    let code = match errno {
        Errno::ENOENT => Code::NotFound,
        Errno::EEXIST => Code::AlreadyExists,
        Errno::EACCES | Errno::EPERM => Code::PermissionDenied,
        Errno::EINVAL => Code::InvalidArgument,
        Errno::ENOSPC => Code::ResourceExhausted,
        Errno::ETIMEDOUT => Code::DeadlineExceeded,
        Errno::EBUSY | Errno::ESTALE | Errno::ENOTSUP | Errno::EROFS => Code::FailedPrecondition,
        _ => Code::Internal,
    };
    let mut status = Status::new(code, message);
    status.metadata_mut().insert(ERRNO, errno.raw().into());
    status
}

/// The reply to a call that did, or failed with an errno.
/// What StatStore says of a store that is and holds `state`.
fn stat_response(state: stores::StoreState) -> StatStoreResponse {
    let upload = state.upload.unwrap_or(extent_store::upload::UploadStatus {
        lease: None,
        uploaded: 0,
        pending_bytes: 0,
        oldest_pending: None,
        stalls: 0,
    });
    let millis = |time: std::time::SystemTime| {
        time.duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64)
    };
    let waited = |since: std::time::SystemTime| {
        since.elapsed().map_or(0, |waited| waited.as_millis() as u64)
    };
    StatStoreResponse {
        branch: state.branch.id,
        tenant: state.branch.cell.tenant,
        cell: state.branch.cell.id,
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

/// The branch a response names by its tenant, cell and ID (TASK-364).
fn branched(tenant: u64, cell: u64, id: u64) -> crate::Branch {
    crate::Branch { cell: crate::Cell { tenant, id: cell }, id }
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

impl Control {
    /// The tenant of a new cell whose store is made at `path`, that the caller names `tenant`
    /// (TASK-364): in a granted tenant's directory, that tenant's, which `tenant` is or is 0 for
    /// (`EPERM` otherwise); elsewhere, `tenant`. A store of a cell there already, a point the VM
    /// manager gives a tenant to restore from (TASK-154), may be another tenant's: the tenant's VM
    /// processes write no store of it (`Tenant::reaches`), and its restores are of their own.
    fn new_cell_at(&self, path: &Path, tenant: u64) -> Result<u64, Status> {
        match self.grants.tenant_at(path) {
            Some(granted) if tenant == crate::host_store::NO_TENANT || tenant == granted => {
                Ok(granted)
            }
            Some(_) => Err(status(Errno::EPERM)),
            None => Ok(tenant),
        }
    }
}

#[tonic::async_trait]
impl Extentd for Control {
    async fn create_store(
        &self,
        request: Request<CreateStoreRequest>,
    ) -> Result<Response<CreateStoreResponse>, Status> {
        let CreateStoreRequest { path: store, capacity, branch, token, tenant, cell, kind } =
            request.into_inner();
        let store = path(store)?;
        let kind = match VolumeKind::try_from(kind) {
            Ok(VolumeKind::Fxfs) => Kind::Fxfs,
            Ok(VolumeKind::Sqlite) => Kind::Sqlite,
            Err(_) => return Err(status(Errno::EINVAL)),
        };
        let tenant = match cell {
            0 => self.new_cell_at(&store, tenant)?,
            _ => tenant,
        };
        let branch = crate::Branch { cell: crate::Cell { tenant, id: cell }, id: branch };
        let made = stores::create(store, capacity, branch, (token, kind)).await;
        reply(made.map(|made| CreateStoreResponse {
            branch: made.id,
            tenant: made.cell.tenant,
            cell: made.cell.id,
        }))
    }

    async fn import_image(
        &self,
        request: Request<ImportImageRequest>,
    ) -> Result<Response<ImportImageResponse>, Status> {
        let ImportImageRequest { path: store, capacity, image, reference, tenant } =
            request.into_inner();
        let (store, image) = (path(store)?, path(image)?);
        let reference = Some(reference).filter(|reference| !reference.is_empty());
        let tenant = self.new_cell_at(&store, tenant)?;
        let made = stores::import(store, capacity, image, reference, tenant).await;
        reply(made.map(|made| ImportImageResponse {
            branch: made.id,
            tenant: made.cell.tenant,
            cell: made.cell.id,
        }))
    }

    async fn fork_store(
        &self,
        request: Request<ForkStoreRequest>,
    ) -> Result<Response<ForkStoreResponse>, Status> {
        let ForkStoreRequest { from, to, point } = request.into_inner();
        let (from, to) = (path(from)?, path(to)?);
        let tenant = self.grants.tenant_at(&to);
        let forked = stores::fork(from, to, tenant, point).await;
        reply(forked.map(|forked| ForkStoreResponse {
            branch: forked.id,
            tenant: forked.cell.tenant,
            cell: forked.cell.id,
        }))
    }

    async fn delete_store(
        &self,
        request: Request<DeleteStoreRequest>,
    ) -> Result<Response<DeleteStoreResponse>, Status> {
        let store = path(request.into_inner().path)?;
        reply(stores::delete(store).await.map(|()| DeleteStoreResponse {}))
    }

    async fn drop_store(
        &self,
        request: Request<DropStoreRequest>,
    ) -> Result<Response<DropStoreResponse>, Status> {
        let store = path(request.into_inner().path)?;
        reply(stores::drop_copy(store).await.map(|()| DropStoreResponse {}))
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
        let MakePublicRequest { path: store, generation } = request.into_inner();
        let store = path(store)?;
        reply(stores::make_public(store, generation).await.map(|()| MakePublicResponse {}))
    }

    async fn attach_base(
        &self,
        request: Request<AttachBaseRequest>,
    ) -> Result<Response<AttachBaseResponse>, Status> {
        let AttachBaseRequest { store, base } = request.into_inner();
        let (store, base) = (path(store)?, path(base)?);
        let grants = self.grants.clone();
        let allowed = move |store: &Path, base: &Path| grants.same_tenant(store, base);
        reply(stores::attach(store, base, allowed).await.map(|()| AttachBaseResponse {}))
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
            // The VM processes' changes to their windows, as they told the daemon, which its
            // stats file and metrics report: not the API.
            stage_ops: _,
            stage_ns: _,
            commits,
            commit_ns,
            commit_store_ns,
            commit_wait_ns,
            segment_closes,
            segment_close_ns,
            // fxdax: the maps the daemon sent the VM processes' windows, which its stats file and
            // metrics report: not the API (TASK-108.1).
            stage_maps: _,
            stage_map_bytes: _,
            // And the VM processes' changes that took access away, as they told it (TASK-91.16).
            stage_overwrites: _,
            stage_leave_outs: _,
            // And the extent cache's trims (TASK-109.2).
            cache_trims: _,
            cache_trim_ns: _,
            // And the faults by kind (TASK-111.1).
            fault_stores: _,
            fault_refusals: _,
            fault_waits: _,
            // And the windows' runs of pages (TASK-131.1).
            stage_segments: _,
            // And the runs of private pages they made (TASK-164.10).
            stage_private_runs: _,
            stage_private_pages: _,
            stage_private_ns: _,
            vm_private_requests: _,
            vm_private_request_ns: _,
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
            failures: (Request::ALL.into_iter())
                .map(|request| (request.name().to_owned(), bucket.failures(request)))
                .collect(),
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
            commit_store_ns,
            commit_wait_ns,
            segment_closes,
            segment_close_ns,
        }))
    }

    async fn get_snapshot(
        &self,
        request: Request<GetSnapshotRequest>,
    ) -> Result<Response<GetSnapshotResponse>, Status> {
        let wait = match request.into_inner().wait_ms {
            0 => super::snapshot::WAIT,
            ms => std::time::Duration::from_millis(ms.into()),
        };
        Ok(Response::new(snapshot_response(super::snapshot::take(wait).await)))
    }

    async fn grant(
        &self,
        request: Request<GrantRequest>,
    ) -> Result<Response<GrantResponse>, Status> {
        let GrantRequest { tenant, dir, uid, gid, id } = request.into_inner();
        let dir = path(dir)?;
        let grants = self.grants.clone();
        let granted = stores::blocking(move || {
            let socket = grants.grant(&tenant, id, &dir, (uid, gid))?;
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

    async fn set_memory_weight(
        &self,
        request: Request<SetMemoryWeightRequest>,
    ) -> Result<Response<SetMemoryWeightResponse>, Status> {
        let SetMemoryWeightRequest { tenant, weight } = request.into_inner();
        reply(memory::set_weight(&tenant, weight).map(|()| SetMemoryWeightResponse {}))
    }

    async fn set_tenant_room(
        &self,
        request: Request<SetTenantRoomRequest>,
    ) -> Result<Response<SetTenantRoomResponse>, Status> {
        let SetTenantRoomRequest { tenant, bytes } = request.into_inner();
        let tenant = Some(tenant).filter(|tenant| !tenant.is_empty());
        reply(memory::set_room(tenant.as_deref(), bytes).map(|()| SetTenantRoomResponse {}))
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

    async fn set_disk_limit(
        &self,
        request: Request<SetDiskLimitRequest>,
    ) -> Result<Response<SetDiskLimitResponse>, Status> {
        let SetDiskLimitRequest { tenant, bytes } = request.into_inner();
        let tenant = Some(tenant).filter(|tenant| !tenant.is_empty());
        reply(disk::set_limit(tenant.as_deref(), bytes).map(|()| SetDiskLimitResponse {}))
    }

    async fn get_tenant_disk(
        &self,
        request: Request<GetTenantDiskRequest>,
    ) -> Result<Response<GetTenantDiskResponse>, Status> {
        let tenant = Some(request.into_inner().tenant).filter(|tenant| !tenant.is_empty());
        let counted = stores::blocking(move || disk::count(tenant.as_deref()));
        let response = |(bytes, limit_bytes)| GetTenantDiskResponse { bytes, limit_bytes };
        reply(counted.await.map(response))
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

    /// Makes an empty store at `path` of `capacity` bytes, of `branch`, an Fxfs volume of its cell:
    /// of a new cell if its cell's ID is 0, and a new branch there if its ID is; placed at `token`
    /// if given; and returns its branch (`CreateStore`, TASK-364).
    pub fn create_store(
        &self,
        path: &Path,
        capacity: u64,
        branch: crate::Branch,
        token: Option<u64>,
    ) -> Result<crate::Branch, crate::Error> {
        let request = CreateStoreRequest {
            path: name(path)?,
            capacity,
            branch: branch.id,
            token,
            tenant: branch.cell.tenant,
            cell: branch.cell.id,
            kind: VolumeKind::Fxfs.into(),
        };
        let response = self
            .call(format!("making the store at {}", path.display()), |mut c| async move {
                c.create_store(request).await
            })?;
        Ok(branched(response.tenant, response.cell, response.branch))
    }

    /// Makes a store at `path` of `capacity` bytes holding the container image at `image`, the one
    /// `reference` names in it if given, the one volume of a new cell of no tenant's, and returns
    /// its branch (`ImportImage`).
    pub fn import_image(
        &self,
        path: &Path,
        capacity: u64,
        image: &Path,
        reference: Option<&str>,
    ) -> Result<crate::Branch, crate::Error> {
        let request = ImportImageRequest {
            path: name(path)?,
            capacity,
            image: name(image)?,
            reference: reference.unwrap_or_default().to_owned(),
            tenant: crate::host_store::NO_TENANT,
        };
        let response = self.call(format!("importing {}", image.display()), |mut c| async move {
            c.import_image(request).await
        })?;
        Ok(branched(response.tenant, response.cell, response.branch))
    }

    /// Forks the store at `from` to a new branch at `to`, and returns the branch (`ForkStore`).
    pub fn fork_store(&self, from: &Path, to: &Path) -> Result<crate::Branch, crate::Error> {
        self.fork_or_point(from, to, false)
    }

    /// Makes a point of the store at `from`, at `to`, and returns its branch (`ForkStore` with
    /// `point`): which any number of hosts fork at once, from copies of it (TASK-158).
    pub fn point_store(&self, from: &Path, to: &Path) -> Result<crate::Branch, crate::Error> {
        self.fork_or_point(from, to, true)
    }

    fn fork_or_point(
        &self,
        from: &Path,
        to: &Path,
        point: bool,
    ) -> Result<crate::Branch, crate::Error> {
        let request = ForkStoreRequest { from: name(from)?, to: name(to)?, point };
        let what = format!("forking the store at {} to {}", from.display(), to.display());
        let response = self.call(what, |mut c| async move { c.fork_store(request).await })?;
        Ok(branched(response.tenant, response.cell, response.branch))
    }

    /// Deletes the store at `path` (`DeleteStore`).
    pub fn delete_store(&self, path: &Path) -> Result<(), crate::Error> {
        let request = DeleteStoreRequest { path: name(path)? };
        let what = format!("deleting the store at {}", path.display());
        self.call(what, |mut c| async move { c.delete_store(request).await }).map(drop)
    }

    /// Deletes this host's copy of the store at `path`, its branch staying in the daemon's bucket
    /// (`DropStore`).
    pub fn drop_store(&self, path: &Path) -> Result<(), crate::Error> {
        let request = DropStoreRequest { path: name(path)? };
        let what = format!("dropping the store at {}", path.display());
        self.call(what, |mut c| async move { c.drop_store(request).await }).map(drop)
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
        let request = MakePublicRequest { path: name(path)?, generation: false };
        let what = format!("making the store at {} public", path.display());
        self.call(what, |mut c| async move { c.make_public(request).await }).map(drop)
    }

    /// Makes the fork at `path` a public generation: its branch alone (`MakePublic`).
    pub fn make_generation_public(&self, path: &Path) -> Result<(), crate::Error> {
        let request = MakePublicRequest { path: name(path)?, generation: true };
        let what = format!("making the generation at {} public", path.display());
        self.call(what, |mut c| async move { c.make_public(request).await }).map(drop)
    }

    /// Attaches the fork at `base` to the store at `path` (`AttachBase`).
    pub fn attach_base(&self, path: &Path, base: &Path) -> Result<(), crate::Error> {
        let request = AttachBaseRequest { store: name(path)?, base: name(base)? };
        let what = format!("attaching {} to {}", base.display(), path.display());
        self.call(what, |mut c| async move { c.attach_base(request).await }).map(drop)
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

    /// The branch of the store at `path`, in its cell (`StatStore`'s `tenant`, `cell` and
    /// `branch`, TASK-364).
    pub fn branch(&self, path: &Path) -> Result<crate::Branch, crate::Error> {
        let stat = self.stat_store(path)?;
        Ok(branched(stat.tenant, stat.cell, stat.branch))
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

    /// Grants the tenant `tenant`, of the ID `id` in the bucket (TASK-364), whose stores are in
    /// `dir`, its VM socket, given to `uid` and `gid` where named, and returns its path (`Grant`).
    pub fn grant(
        &self,
        (tenant, id): (&str, u64),
        dir: &Path,
        uid: Option<u32>,
        gid: Option<u32>,
    ) -> Result<PathBuf, crate::Error> {
        let request = GrantRequest { tenant: tenant.to_owned(), dir: name(dir)?, uid, gid, id };
        let response =
            self.call(format!("granting {tenant}"), |mut c| async move { c.grant(request).await })?;
        Ok(response.socket.into())
    }

    /// Revokes the tenant `tenant`'s grant: its VM socket goes, its VM processes connected keep
    /// their stores (`Revoke`).
    pub fn revoke(&self, tenant: &str) -> Result<(), crate::Error> {
        let request = RevokeRequest { tenant: tenant.to_owned() };
        let what = format!("revoking {tenant}");
        self.call(what, |mut c| async move { c.revoke(request).await }).map(drop)
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

    /// fxdax: weighs the memory of the tenant `tenant` against the others' (`SetMemoryWeight`,
    /// TASK-358).
    pub fn set_memory_weight(&self, tenant: &str, weight: u64) -> Result<(), crate::Error> {
        let request = SetMemoryWeightRequest { tenant: tenant.to_owned(), weight };
        let what = format!("weighing the memory of {tenant}");
        self.call(what, |mut c| async move { c.set_memory_weight(request).await }).map(drop)
    }

    /// fxdax: sets the room the connections of the tenant `tenant`, or of every tenant without one,
    /// share to `bytes`, or to the default without them (`SetTenantRoom`, TASK-149.35).
    pub fn set_tenant_room(
        &self,
        tenant: Option<&str>,
        bytes: Option<u64>,
    ) -> Result<(), crate::Error> {
        let request = SetTenantRoomRequest { tenant: tenant.unwrap_or_default().to_owned(), bytes };
        let what = format!("setting the room of {}", tenant.unwrap_or("every tenant"));
        self.call(what, |mut c| async move { c.set_tenant_room(request).await }).map(drop)
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

    /// fxdax: limits the disk of the tenant `tenant`, or of every tenant without one, to `bytes`,
    /// or lifts the limit without them (`SetDiskLimit`, TASK-94).
    pub fn set_disk_limit(
        &self,
        tenant: Option<&str>,
        bytes: Option<u64>,
    ) -> Result<(), crate::Error> {
        let request = SetDiskLimitRequest { tenant: tenant.unwrap_or_default().to_owned(), bytes };
        let what = format!("limiting the disk of {}", tenant.unwrap_or("every tenant"));
        self.call(what, |mut c| async move { c.set_disk_limit(request).await }).map(drop)
    }

    /// fxdax: what the disk of the tenant `tenant`, or of every tenant without one, takes, counted
    /// now, in bytes, and its limit, if it has one (`GetTenantDisk`, TASK-94).
    pub fn tenant_disk(&self, tenant: Option<&str>) -> Result<(u64, Option<u64>), crate::Error> {
        let request = GetTenantDiskRequest { tenant: tenant.unwrap_or_default().to_owned() };
        let what = format!("counting the disk of {}", tenant.unwrap_or("every tenant"));
        let response = self.call(what, |mut c| async move { c.get_tenant_disk(request).await })?;
        Ok((response.bytes, response.limit_bytes))
    }

    /// fxdax: what the daemon, its VM processes and their guests counted, as of one moment, each
    /// with its build (`GetSnapshot`, TASK-111.4): the VM processes' answers waited for `wait` at
    /// most, the daemon's default if none.
    pub fn snapshot(
        &self,
        wait: Option<std::time::Duration>,
    ) -> Result<GetSnapshotResponse, crate::Error> {
        let wait_ms = wait.map_or(0, |wait| u32::try_from(wait.as_millis()).unwrap_or(u32::MAX));
        self.call("asking the extent daemon for a snapshot".to_owned(), |mut c| async move {
            c.get_snapshot(GetSnapshotRequest { wait_ms }).await
        })
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
            // The VM processes' changes to their windows, which the API doesn't report
            // (`Stats::stage_ops`).
            stage_ops: 0,
            stage_ns: 0,
            commits: response.commits,
            commit_ns: response.commit_ns,
            commit_store_ns: response.commit_store_ns,
            commit_wait_ns: response.commit_wait_ns,
            segment_closes: response.segment_closes,
            segment_close_ns: response.segment_close_ns,
            // The maps the daemon sent the VM processes' windows, which the API doesn't report.
            stage_maps: 0,
            stage_map_bytes: 0,
            stage_overwrites: 0,
            stage_leave_outs: 0,
            cache_trims: 0,
            cache_trim_ns: 0,
            fault_stores: 0,
            fault_refusals: 0,
            fault_waits: 0,
            stage_segments: 0,
            stage_private_runs: 0,
            stage_private_pages: 0,
            stage_private_ns: 0,
            vm_private_requests: 0,
            vm_private_request_ns: 0,
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
