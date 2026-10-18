// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The daemon's control API (TASK-43.3): gRPC over a Unix socket, for the host's VM manager, which
//! launches the VMMs. Its calls and their errors are `proto/fxdax/extentd/v1/extentd.proto`'s.
//!
//! The store operations it serves block, on I/O and on waits ([`super::stores`]): each runs on the
//! runtime's blocking threads, so that a long one, a sync while object storage is slow, holds no
//! thread that serves other calls.

use super::stores;
use crate::Errno;
use proto::extentd_client::ExtentdClient;
use proto::extentd_server::{Extentd, ExtentdServer};
use proto::*;
use std::path::{Path, PathBuf};
use tonic::{Code, Request, Response, Status};

/// The API's messages and services, generated from its .proto.
pub mod proto {
    tonic::include_proto!("fxdax.extentd.v1");
}

/// The metadata entry of a failed call's status holding the errno it failed with, a decimal
/// number.
pub const ERRNO: &str = "fxdax-errno";

/// How many threads serve the API's calls; their work runs on blocking threads of its own.
const THREADS: usize = 2;

/// Serves the control API to the callers that connect to `listener`, until accepting fails.
pub fn serve_control(listener: std::os::unix::net::UnixListener) -> std::io::Result<()> {
    listener.set_nonblocking(true)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(THREADS)
        .thread_name("extentd control")
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let incoming = tokio_stream::wrappers::UnixListenerStream::new(
            tokio::net::UnixListener::from_std(listener)?,
        );
        tonic::transport::Server::builder()
            .add_service(ExtentdServer::new(Control))
            .serve_with_incoming(incoming)
            .await
            .map_err(std::io::Error::other)
    })
}

/// A client of the control API of the daemon listening at `socket`.
pub async fn connect(
    socket: impl AsRef<Path>,
) -> Result<ExtentdClient<tonic::transport::Channel>, tonic::transport::Error> {
    let socket = socket.as_ref().to_owned();
    // The URI names no host: the connector connects to the socket, whatever it asks for.
    let channel = tonic::transport::Endpoint::from_static("http://extentd")
        .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
            let socket = socket.clone();
            async move {
                let stream = tokio::net::UnixStream::connect(socket).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
            }
        }))
        .await?;
    Ok(ExtentdClient::new(channel))
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

/// Runs `work`, which blocks, on a blocking thread, and replies with what it returns.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Errno> + Send + 'static,
) -> Result<Response<T>, Status> {
    match tokio::task::spawn_blocking(work).await {
        Ok(done) => done.map(Response::new).map_err(status),
        Err(err) => {
            log::error!("extentd: a control call's work: {err}");
            Err(Status::internal("the call's work panicked"))
        }
    }
}

/// The store a request names by `path`.
fn path(path: String) -> Result<PathBuf, Status> {
    if path.is_empty() {
        return Err(status(Errno::EINVAL));
    }
    Ok(PathBuf::from(path))
}

/// The API's calls, served from the daemon's stores.
struct Control;

#[tonic::async_trait]
impl Extentd for Control {
    async fn create_store(
        &self,
        request: Request<CreateStoreRequest>,
    ) -> Result<Response<CreateStoreResponse>, Status> {
        let CreateStoreRequest { path: store, capacity, branch, token } = request.into_inner();
        let store = path(store)?;
        blocking(move || {
            let branch = stores::create(&store, capacity, Some(branch), token)?;
            Ok(CreateStoreResponse { branch })
        })
        .await
    }

    async fn import_image(
        &self,
        request: Request<ImportImageRequest>,
    ) -> Result<Response<ImportImageResponse>, Status> {
        let ImportImageRequest { path: store, capacity, image, reference } = request.into_inner();
        let (store, image) = (path(store)?, path(image)?);
        blocking(move || {
            let reference = Some(reference.as_str()).filter(|reference| !reference.is_empty());
            let branch = stores::import(&store, capacity, &image, reference)?;
            Ok(ImportImageResponse { branch })
        })
        .await
    }

    async fn fork_store(
        &self,
        request: Request<ForkStoreRequest>,
    ) -> Result<Response<ForkStoreResponse>, Status> {
        let ForkStoreRequest { from, to, branch } = request.into_inner();
        let (from, to) = (path(from)?, path(to)?);
        let branch = if branch == 0 { rand::random::<u64>().max(1) } else { branch };
        blocking(move || {
            stores::fork(&from, &to, branch)?;
            Ok(ForkStoreResponse { branch })
        })
        .await
    }

    async fn delete_store(
        &self,
        request: Request<DeleteStoreRequest>,
    ) -> Result<Response<DeleteStoreResponse>, Status> {
        let store = path(request.into_inner().path)?;
        blocking(move || stores::delete(&store).map(|()| DeleteStoreResponse {})).await
    }

    async fn resize_store(
        &self,
        request: Request<ResizeStoreRequest>,
    ) -> Result<Response<ResizeStoreResponse>, Status> {
        let ResizeStoreRequest { path: store, capacity } = request.into_inner();
        let store = path(store)?;
        blocking(move || stores::resize(&store, capacity).map(|()| ResizeStoreResponse {})).await
    }

    async fn place_store(
        &self,
        request: Request<PlaceStoreRequest>,
    ) -> Result<Response<PlaceStoreResponse>, Status> {
        let PlaceStoreRequest { path: store, token } = request.into_inner();
        let store = path(store)?;
        blocking(move || stores::place(&store, token).map(|()| PlaceStoreResponse {})).await
    }

    async fn make_public(
        &self,
        request: Request<MakePublicRequest>,
    ) -> Result<Response<MakePublicResponse>, Status> {
        let store = path(request.into_inner().path)?;
        blocking(move || stores::make_public(&store).map(|()| MakePublicResponse {})).await
    }

    async fn sync_store(
        &self,
        request: Request<SyncStoreRequest>,
    ) -> Result<Response<SyncStoreResponse>, Status> {
        let store = path(request.into_inner().path)?;
        blocking(move || stores::sync(&store).map(|()| SyncStoreResponse {})).await
    }

    async fn stat_store(
        &self,
        request: Request<StatStoreRequest>,
    ) -> Result<Response<StatStoreResponse>, Status> {
        let store = path(request.into_inner().path)?;
        blocking(move || {
            let state = stores::stat(&store)?;
            Ok(StatStoreResponse {
                branch: state.branch,
                capacity: state.info.capacity,
                used: state.info.used,
                available: state.info.available,
                public: state.public,
                open: state.open,
            })
        })
        .await
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
        } = crate::stats::stats();
        let object_storage = stores::remote_bucket().map_err(status)?.map(|bucket| {
            use extent_remote::store::{Request, Upload};
            let stats = bucket.stats();
            ObjectStorageStats {
                requests: (Request::ALL.into_iter())
                    .map(|request| (request.name().to_owned(), stats.requests(request)))
                    .collect(),
                uploaded_bytes: (Upload::ALL.into_iter())
                    .map(|upload| (upload.name().to_owned(), stats.uploaded(upload)))
                    .collect(),
                downloaded_bytes: stats.downloaded,
            }
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
        }))
    }
}
