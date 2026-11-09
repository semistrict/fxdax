// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The control API of a FUSE mount of many stores (TASK-302): gRPC over a Unix socket, whose calls
//! and errors are `proto/fxdax/mount/v1/mount.proto`'s. It attaches branches of the mount's
//! bucket, each taken into a store in the mount's directory of stores, and detaches them.
//!
//! An attach or detach runs in a task of its own, which a caller giving up on it doesn't stop:
//! a store is attached or detached whole, or not at all.

use super::server::Server;
use crate::error::{Errno, Error};
use crate::extentd::control::{channel, errno, saying};
use crate::{Filesystem, SetAttr, Storage};
use proto::mount_client::MountClient;
use proto::mount_server::MountServer;
use proto::*;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tonic::{Request, Response, Status};

/// The API's messages and services, generated from its .proto.
pub mod proto {
    tonic::include_proto!("fxdax.mount.v1");
}

/// How many threads serve the API's calls; the stores' opening and closing run on blocking
/// threads of their own.
const THREADS: usize = 2;

/// Where a mount of many stores takes the branches it attaches from, and keeps their stores.
#[derive(Clone, Debug)]
pub struct Stores {
    /// The directory the stores are in, each at the name it is attached at.
    pub dir: PathBuf,
    /// The URL of the bucket the branches are in.
    pub bucket: String,
}

/// Serves the control API of `server`, a server of many stores, which attaches stores as `stores`
/// says, to the callers that connect to `listener`, until accepting fails.
pub fn serve(
    listener: std::os::unix::net::UnixListener,
    server: Arc<Server>,
    stores: Stores,
) -> std::io::Result<()> {
    listener.set_nonblocking(true)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(THREADS)
        .thread_name("fuse control")
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let incoming = tokio_stream::wrappers::UnixListenerStream::new(
            tokio::net::UnixListener::from_std(listener)?,
        );
        let control = Control { server, stores: Arc::new(stores) };
        tonic::transport::Server::builder()
            .add_service(MountServer::new(control))
            .serve_with_incoming(incoming)
            .await
            .map_err(std::io::Error::other)
    })
}

/// The API's calls, served by a server of many stores.
struct Control {
    server: Arc<Server>,
    stores: Arc<Stores>,
}

impl Control {
    /// The directory of the store attached, or to be, at `name`: `EINVAL` for a name no
    /// directory holds, which can't name another's.
    fn store(&self, name: &str) -> Result<PathBuf, Status> {
        crate::filesystem::valid_name(name.as_bytes()).map_err(|errno| failed(errno.into()))?;
        Ok(self.stores.dir.join(name))
    }
}

#[tonic::async_trait]
impl proto::mount_server::Mount for Control {
    async fn attach(
        &self,
        request: Request<AttachRequest>,
    ) -> Result<Response<AttachResponse>, Status> {
        let AttachRequest { name, branch, capacity, token } = request.into_inner();
        let path = self.store(&name)?;
        let (server, bucket) = (self.server.clone(), self.stores.bucket.clone());
        let attached = whole(async move {
            let mut taken = None;
            let open = async {
                let opened = tokio::task::spawn_blocking(move || {
                    let (storage, branch, new) =
                        Storage::take(&path, (branch, capacity), &bucket, token)?;
                    let mounting = async {
                        if !new {
                            return Filesystem::mount(storage).await;
                        }
                        // A new store's root is the mounting user's, as the mount is.
                        let fs = Filesystem::format(storage).await?;
                        // SAFETY: Plain calls returning the process's IDs.
                        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
                        let owner =
                            SetAttr { uid: Some(uid), gid: Some(gid), ..SetAttr::default() };
                        fs.root().setattr(&owner).await?;
                        Ok(fs)
                    };
                    let fs = futures::executor::block_on(mounting)
                        .map_err(|err| err.context(format!("mounting {}", path.display())))?;
                    Ok::<_, Error>((fs, branch))
                });
                let (fs, branch) = opened.await.map_err(Error::new)??;
                taken = Some(branch);
                Ok(fs)
            };
            server.attach(&name, open).await?;
            Ok(taken.expect("attached once opened"))
        });
        Ok(Response::new(AttachResponse { branch: attached.await? }))
    }

    async fn detach(
        &self,
        request: Request<DetachRequest>,
    ) -> Result<Response<DetachResponse>, Status> {
        let DetachRequest { name, force } = request.into_inner();
        let server = self.server.clone();
        whole(async move { server.detach(&name, force).await }).await?;
        Ok(Response::new(DetachResponse {}))
    }

    async fn list(&self, _: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        let (attached, dir) = (self.server.attached(), self.stores.dir.clone());
        let listed = tokio::task::spawn_blocking(move || {
            (attached.into_iter())
                .map(|attached| {
                    let store = dir.join(&attached.name);
                    let branch = extent_store::host_branch(&store).unwrap_or_else(|err| {
                        log::error!("fuse: the branch of {}: {err:#}", store.display());
                        0
                    });
                    proto::Attached { name: attached.name, branch, detaching: attached.detaching }
                })
                .collect()
        });
        let stores = listed.await.map_err(|err| failed(Error::new(err)))?;
        Ok(Response::new(ListResponse { stores }))
    }
}

/// Runs `work` in a task of its own, which completes though the caller gives up on it, and its
/// result as the API's.
async fn whole<T: Send + 'static>(
    work: impl Future<Output = Result<T, Error>> + Send + 'static,
) -> Result<T, Status> {
    match tokio::spawn(work).await {
        Ok(done) => done.map_err(failed),
        Err(err) => Err(failed(Error::new(err))),
    }
}

/// The status of a call that failed with `err`: its errno's, `EIO` if it has none, saying what
/// failed.
fn failed(err: Error) -> Status {
    let errno = err.errno().unwrap_or(Errno::EIO);
    if errno == Errno::EIO {
        log::error!("fuse: {err:#}");
    }
    saying(errno, format!("{err:#}"))
}

/// A client of the control API of a FUSE mount of many stores that blocks, for callers without a
/// runtime of their own: `fxdax-mount`'s commands, and tests. Its errors carry the errno a call
/// failed with, if it did ([`crate::Error::errno`]).
pub struct Client {
    runtime: tokio::runtime::Runtime,
    client: MountClient<tonic::transport::Channel>,
}

impl Client {
    /// A client of the mount whose control socket is at `socket`.
    pub fn connect(socket: impl AsRef<Path>) -> Result<Self, Error> {
        let socket = socket.as_ref();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(Error::new)?;
        let channel = runtime.block_on(channel(socket)).map_err(|err| {
            Error::new(anyhow::Error::new(err).context(format!(
                "connecting to the mount's control socket at {}",
                socket.display()
            )))
        })?;
        Ok(Self { runtime, client: MountClient::new(channel) })
    }

    /// Attaches `branch`, or a new branch if 0, at `name`, a store of `capacity` bytes if it is
    /// made, its lease taken at `token`'s placement if given, and returns the branch (`Attach`).
    pub fn attach(
        &self,
        name: &str,
        (branch, capacity): (u64, u64),
        token: Option<u64>,
    ) -> Result<u64, Error> {
        let request = AttachRequest { name: name.to_owned(), branch, capacity, token };
        let response = self.call(format!("attaching {name}"), |mut client| async move {
            client.attach(request).await
        })?;
        Ok(response.branch)
    }

    /// Detaches the store attached at `name`, at once if `force` (`Detach`).
    pub fn detach(&self, name: &str, force: bool) -> Result<(), Error> {
        let request = DetachRequest { name: name.to_owned(), force };
        self.call(format!("detaching {name}"), |mut client| async move {
            client.detach(request).await
        })?;
        Ok(())
    }

    /// The stores attached, and those being detached (`List`).
    pub fn list(&self) -> Result<Vec<proto::Attached>, Error> {
        let response = self.call("listing the stores".to_owned(), |mut client| async move {
            client.list(ListRequest {}).await
        })?;
        Ok(response.stores)
    }

    fn call<T, F>(
        &self,
        what: String,
        call: impl FnOnce(MountClient<tonic::transport::Channel>) -> F,
    ) -> Result<T, Error>
    where
        F: Future<Output = Result<Response<T>, Status>>,
    {
        match self.runtime.block_on(call(self.client.clone())) {
            Ok(response) => Ok(response.into_inner()),
            Err(status) => Err(Error::new(match errno(&status) {
                Some(errno) => anyhow::Error::new(status).context(errno).context(what),
                None => anyhow::Error::new(status).context(what),
            })),
        }
    }
}
