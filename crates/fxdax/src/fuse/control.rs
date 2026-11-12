// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The control API of a FUSE mount of many stores (TASK-302): gRPC over a Unix socket, whose calls
//! and errors are `proto/fxdax/mount/v1/mount.proto`'s. It attaches tenants' cells of the mount's
//! bucket (TASK-364), each Fxfs volume of a cell taken into a store in the mount's directory of
//! stores, at `TENANT/CELL/VOLUME`, and shown at that path in the mount; and detaches them.
//!
//! An attach or detach runs in a task of its own, which a caller giving up on it doesn't stop:
//! a cell is attached or detached whole, or not at all.

use super::server::Server;
use crate::error::{Errno, Error};
use crate::extentd::control::{channel, errno, saying};
use crate::{Branch, Cell, Filesystem, SetAttr, Storage};
use extent_remote::cell::{self, Kind};
use proto::mount_client::MountClient;
use proto::mount_server::MountServer;
use proto::*;
use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tonic::{Request, Response, Status};

/// The API's messages and services, generated from its .proto.
pub mod proto {
    tonic::include_proto!("fxdax.mount.v1");
}

/// How many threads serve the API's calls; the stores' opening and closing run on blocking
/// threads of their own.
const THREADS: usize = 2;

/// Where a mount of many stores takes the cells it attaches from, and keeps their stores.
#[derive(Clone, Debug)]
pub struct Stores {
    /// The directory the stores are in, each at the path it is attached at.
    pub dir: PathBuf,
    /// The URL of the bucket the cells are in.
    pub bucket: String,
}

/// Serves the control API of `server`, a server of many stores, which attaches cells as `stores`
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
        let control = Control { server, stores: Arc::new(stores), cells: Arc::default() };
        tonic::transport::Server::builder()
            .add_service(MountServer::new(control))
            .serve_with_incoming(incoming)
            .await
            .map_err(std::io::Error::other)
    })
}

/// The path a volume of `cell` named `name` has in the mount, and in its directory of stores:
/// `TENANT/CELL/NAME`, the tenant and the cell in 16 hexadecimal digits, as the bucket's keys
/// have them.
pub fn volume_path(cell: Cell, name: &str) -> String {
    format!("{:016x}/{:016x}/{name}", cell.tenant, cell.id)
}

/// The cells attached, or being attached or detached, and the fencing token each was taken at.
type Cells = Arc<Mutex<BTreeMap<Cell, Attaching>>>;

/// A cell of the mount's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Attaching {
    /// Being attached.
    Busy,
    /// Attached, taken at the placement of this token, if one was given.
    Attached(Option<u64>),
    /// Being detached: listed, its volumes as they go.
    Detaching,
}

/// The API's calls, served by a server of many stores.
struct Control {
    server: Arc<Server>,
    stores: Arc<Stores>,
    cells: Cells,
}

/// The cell `cell` of `cells` as `reserving` finds it: marked `marked` if it says so, else left be.
fn reserve(
    cells: &Cells,
    (cell, marked): (Cell, Attaching),
    reserving: impl FnOnce(Option<Attaching>) -> Result<(), Errno>,
) -> Result<(), Errno> {
    let mut cells = cells.lock().unwrap();
    reserving(cells.get(&cell).copied())?;
    cells.insert(cell, marked);
    Ok(())
}

/// Takes `branch`, a volume of its cell, into a store at `path` in the bucket at `url`, of
/// `capacity` bytes until its branch says its own, placed at `token` if given, and mounts its
/// filesystem: made empty if `new`, its root the mounting user's, as the mount is.
fn mount_volume(
    path: &Path,
    url: &str,
    (branch, capacity): (Branch, u64),
    token: Option<u64>,
    new: bool,
) -> Result<Filesystem, Error> {
    // Its tenant's and its cell's directories, made once for all their volumes.
    if let Some(cell) = path.parent() {
        std::fs::create_dir_all(cell)
            .map_err(|err| Error::new(err).context(format!("making {}", cell.display())))?;
    }
    let storage = Storage::take(path, (branch, capacity), url, token)?;
    let mounting = async {
        if !new {
            return Filesystem::mount(storage).await;
        }
        let fs = Filesystem::format(storage).await?;
        // SAFETY: Plain calls returning the process's IDs.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        let owner = SetAttr { uid: Some(uid), gid: Some(gid), ..SetAttr::default() };
        fs.root().setattr(&owner).await?;
        Ok(fs)
    };
    futures::executor::block_on(mounting)
        .map_err(|err| err.context(format!("mounting {}", path.display())))
}

/// The capacity a store taking a volume of a cell is made with, until the volume's branch says
/// its own, which every volume made here does as its filesystem is made: a block.
const TAKEN_CAPACITY: u64 = Storage::BLOCK_SIZE as u64;

impl Control {
    /// Attaches the Fxfs volumes of `cell`'s `object` at their paths, as [`mount_volume`] takes
    /// and mounts them, placed at `token` if given; on a failure, detaches those it attached, at
    /// once, and fails. Returns them, by name.
    async fn attach_volumes(
        &self,
        cell: Cell,
        object: &cell::Object,
        token: Option<u64>,
    ) -> Result<Vec<proto::Volume>, Error> {
        let mut attached = Vec::new();
        for (name, volume) in &object.volumes {
            if volume.kind != Kind::Fxfs {
                continue;
            }
            let shown = volume_path(cell, name);
            let path = self.stores.dir.join(&shown);
            let url = self.stores.bucket.clone();
            let branch = Branch { cell, id: volume.branch };
            let open = async move {
                let opened = tokio::task::spawn_blocking(move || {
                    mount_volume(&path, &url, (branch, TAKEN_CAPACITY), token, false)
                });
                opened.await.map_err(Error::new)?
            };
            if let Err(err) = self.server.attach(&shown, open).await {
                for name in attached.iter().map(|volume: &proto::Volume| &volume.name) {
                    let detached = self.server.detach(&volume_path(cell, name), true).await;
                    if let Err(why) = detached {
                        log::error!("fuse: detaching {name} of a cell attached part way: {why}");
                    }
                }
                return Err(err);
            }
            let named =
                proto::Volume { name: name.clone(), branch: volume.branch, detaching: false };
            attached.push(named);
        }
        Ok(attached)
    }
}

#[tonic::async_trait]
impl proto::mount_server::Mount for Control {
    async fn attach(
        &self,
        request: Request<AttachRequest>,
    ) -> Result<Response<AttachResponse>, Status> {
        let AttachRequest { tenant, cell, token } = request.into_inner();
        // A cell of 0 is a new one, made as it is held; any other is the bucket's, or none.
        let make = cell == 0;
        let cell = match cell {
            0 => crate::host_store::new_cell(tenant),
            id => Cell { tenant, id },
        };
        reserve(&self.cells, (cell, Attaching::Busy), |found| match found {
            None => Ok(()),
            Some(Attaching::Attached(_)) => Err(Errno::EEXIST),
            Some(Attaching::Busy | Attaching::Detaching) => Err(Errno::EBUSY),
        })
        .map_err(|errno| failed(errno.into()))?;
        let control = Control {
            server: self.server.clone(),
            stores: self.stores.clone(),
            cells: self.cells.clone(),
        };
        let attached = whole(async move {
            let attached = async {
                let url = control.stores.bucket.clone();
                let held = tokio::task::spawn_blocking(move || {
                    let bucket = crate::host_store::bucket(&url).map_err(Error::new)?;
                    let placement = match token {
                        Some(token) => token,
                        None => crate::host_store::token().map_err(Error::new)?,
                    };
                    let holding = cell::hold(&*bucket, cell, (placement, placement), make, None);
                    let held = extent_store::upload::block_on(holding).map_err(Error::new)?;
                    let (_, object) = held.map_err(|err| {
                        let errno = cell_errno(&err);
                        Error::new(anyhow::Error::new(err).context(errno))
                    })?;
                    Ok::<_, Error>(object)
                });
                let object = held.await.map_err(Error::new)??;
                control.attach_volumes(cell, &object, token).await
            }
            .await;
            let mut cells = control.cells.lock().unwrap();
            match &attached {
                Ok(_) => cells.insert(cell, Attaching::Attached(token)),
                Err(_) => cells.remove(&cell),
            };
            attached
        });
        let volumes = attached.await?;
        Ok(Response::new(AttachResponse { cell: cell.id, volumes }))
    }

    async fn create_volume(
        &self,
        request: Request<CreateVolumeRequest>,
    ) -> Result<Response<CreateVolumeResponse>, Status> {
        let CreateVolumeRequest { tenant, cell, name, capacity } = request.into_inner();
        let cell = Cell { tenant, id: cell };
        if !cell::valid_name(&name) {
            return Err(failed(Errno::EINVAL.into()));
        }
        let token = match self.cells.lock().unwrap().get(&cell) {
            Some(Attaching::Attached(token)) => *token,
            Some(Attaching::Busy | Attaching::Detaching) => {
                return Err(failed(Errno::EBUSY.into()));
            }
            None => return Err(failed(Errno::ENOENT.into())),
        };
        let shown = volume_path(cell, &name);
        let path = self.stores.dir.join(&shown);
        if path.exists() {
            return Err(failed(Errno::EEXIST.into()));
        }
        let (server, url) = (self.server.clone(), self.stores.bucket.clone());
        let branch = crate::host_store::new_branch(cell);
        let made = whole(async move {
            let open = async move {
                let opened = tokio::task::spawn_blocking(move || {
                    mount_volume(&path, &url, (branch, capacity), token, true)
                });
                opened.await.map_err(Error::new)?
            };
            server.attach(&shown, open).await
        });
        made.await?;
        Ok(Response::new(CreateVolumeResponse { branch: branch.id }))
    }

    async fn detach(
        &self,
        request: Request<DetachRequest>,
    ) -> Result<Response<DetachResponse>, Status> {
        let DetachRequest { tenant, cell, force } = request.into_inner();
        let cell = Cell { tenant, id: cell };
        reserve(&self.cells, (cell, Attaching::Detaching), |found| match found {
            Some(Attaching::Attached(_)) => Ok(()),
            Some(Attaching::Busy | Attaching::Detaching) => Err(Errno::EBUSY),
            None => Err(Errno::ENOENT),
        })
        .map_err(|errno| failed(errno.into()))?;
        let (server, cells) = (self.server.clone(), self.cells.clone());
        let prefix = format!("{:016x}/{:016x}/", cell.tenant, cell.id);
        let detached = whole(async move {
            let shown: Vec<String> = (server.attached().into_iter())
                .map(|attached| attached.name)
                .filter(|name| name.starts_with(&prefix))
                .collect();
            let detaching = shown.iter().map(|name| server.detach(name, force));
            let detached = futures::future::join_all(detaching).await;
            // A store that failed to unmount is gone from the mount all the same
            // (`Server::detach`): the cell is no longer attached, and attaching it again opens its
            // stores anew.
            cells.lock().unwrap().remove(&cell);
            match detached.into_iter().find_map(Result::err) {
                Some(err) => Err(err),
                None => Ok(()),
            }
        });
        detached.await?;
        Ok(Response::new(DetachResponse {}))
    }

    async fn list(&self, _: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        let attached: Vec<Cell> = (self.cells.lock().unwrap().iter())
            .filter(|(_, attaching)| !matches!(attaching, Attaching::Busy))
            .map(|(cell, _)| *cell)
            .collect();
        let (shown, dir) = (self.server.attached(), self.stores.dir.clone());
        let listed = tokio::task::spawn_blocking(move || {
            (attached.into_iter())
                .map(|cell| {
                    let prefix = format!("{:016x}/{:016x}/", cell.tenant, cell.id);
                    let mut volumes: Vec<proto::Volume> = (shown.iter())
                        .filter_map(|attached| {
                            let name = attached.name.strip_prefix(&prefix)?;
                            let store = dir.join(&attached.name);
                            let branch = match extent_store::host_branch(&store) {
                                Ok(branch) => branch.id,
                                Err(err) => {
                                    log::error!("fuse: the branch of {}: {err:#}", store.display());
                                    0
                                }
                            };
                            let detaching = attached.detaching;
                            Some(proto::Volume { name: name.to_owned(), branch, detaching })
                        })
                        .collect();
                    // By name, as the API has them.
                    volumes.sort_by(|one, other| one.name.cmp(&other.name));
                    AttachedCell { tenant: cell.tenant, cell: cell.id, volumes }
                })
                .collect()
        });
        let cells = listed.await.map_err(|err| failed(Error::new(err)))?;
        Ok(Response::new(ListResponse { cells }))
    }
}

/// The errno a failure to hold a cell says, as the extent daemon's API has it: `ENOENT` for a cell
/// the bucket has none of, `ESTALE` for one a later placement holds.
fn cell_errno(err: &extent_remote::Error) -> Errno {
    let failure = extent_store::upload::Failure::Remote(err.clone());
    crate::remote::errno(&failure).unwrap_or(Errno::EIO)
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

    /// Attaches `cell`, or a new cell of its tenant if its ID is 0, its lease taken at `token`'s
    /// placement if given, and returns it and its volumes shown (`Attach`).
    pub fn attach(
        &self,
        cell: Cell,
        token: Option<u64>,
    ) -> Result<(Cell, Vec<proto::Volume>), Error> {
        let request = AttachRequest { tenant: cell.tenant, cell: cell.id, token };
        let response = self
            .call(format!("attaching cell {:x}", cell.id), |mut client| async move {
                client.attach(request).await
            })?;
        Ok((Cell { id: response.cell, ..cell }, response.volumes))
    }

    /// Makes a new Fxfs volume `name` of `capacity` bytes of `cell`, attached, and returns its
    /// branch (`CreateVolume`).
    pub fn create_volume(&self, cell: Cell, name: &str, capacity: u64) -> Result<Branch, Error> {
        let (tenant, id, name) = (cell.tenant, cell.id, name.to_owned());
        let request = CreateVolumeRequest { tenant, cell: id, name: name.clone(), capacity };
        let response = self.call(format!("making volume {name}"), |mut client| async move {
            client.create_volume(request).await
        })?;
        Ok(Branch { cell, id: response.branch })
    }

    /// Detaches `cell`, at once if `force` (`Detach`).
    pub fn detach(&self, cell: Cell, force: bool) -> Result<(), Error> {
        let request = DetachRequest { tenant: cell.tenant, cell: cell.id, force };
        self.call(format!("detaching cell {:x}", cell.id), |mut client| async move {
            client.detach(request).await
        })?;
        Ok(())
    }

    /// The cells attached, and their volumes shown (`List`).
    pub fn list(&self) -> Result<Vec<proto::AttachedCell>, Error> {
        let response = self.call("listing the cells".to_owned(), |mut client| async move {
            client.list(ListRequest {}).await
        })?;
        Ok(response.cells)
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
