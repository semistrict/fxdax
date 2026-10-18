// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An extent daemon serving on threads of a test's process, and a client of its control API.

use fxdax::extentd::control::proto::extentd_client::ExtentdClient;
use fxdax::extentd::control::proto::*;
use fxdax::extentd::control::{connect, errno};
use fxdax::extentd::{serve, serve_control};
use fxdax::{Errno, ExtentId, RemoteExtentStore, VolatileSlice};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use tonic::Code;
use tonic::transport::Channel;

/// A daemon serving in `dir`: its VM socket's path, and a client of its control API, which
/// `runtime` runs.
pub struct Daemon {
    pub vms: PathBuf,
    pub client: ExtentdClient<Channel>,
    pub runtime: tokio::runtime::Runtime,
}

impl Daemon {
    pub fn new(dir: &Path) -> Self {
        let (vms, control) = (dir.join("vms.sock"), dir.join("control.sock"));
        let listener = UnixListener::bind(&vms).unwrap();
        std::thread::spawn(move || serve(listener));
        let listener = UnixListener::bind(&control).unwrap();
        let tenants = dir.join("tenants");
        std::fs::create_dir(&tenants).unwrap();
        std::thread::spawn(move || serve_control(listener, tenants));
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let client = runtime.block_on(connect(&control)).unwrap();
        Self { vms, client, runtime }
    }

    /// A VM process's store at `path`, created with `capacity` bytes if it doesn't exist.
    pub fn open(&self, path: &Path, capacity: u64) -> RemoteExtentStore {
        RemoteExtentStore::connect(&self.vms, path, capacity).unwrap()
    }

    pub fn create(
        &mut self,
        path: &Path,
        branch: u64,
        token: Option<u64>,
    ) -> Result<u64, tonic::Status> {
        let request = CreateStoreRequest { path: name(path), capacity: CAPACITY, branch, token };
        let response = self.runtime.block_on(self.client.create_store(request))?;
        Ok(response.into_inner().branch)
    }

    pub fn stat(&mut self, path: &Path) -> Result<StatStoreResponse, tonic::Status> {
        let request = StatStoreRequest { path: name(path) };
        Ok(self.runtime.block_on(self.client.stat_store(request))?.into_inner())
    }

    pub fn fork(&mut self, from: &Path, to: &Path) -> Result<u64, tonic::Status> {
        let request = ForkStoreRequest { from: name(from), to: name(to), branch: 0 };
        Ok(self.runtime.block_on(self.client.fork_store(request))?.into_inner().branch)
    }

    pub fn resize(&mut self, path: &Path, capacity: u64) -> Result<(), tonic::Status> {
        let request = ResizeStoreRequest { path: name(path), capacity };
        self.runtime.block_on(self.client.resize_store(request)).map(drop)
    }

    pub fn place(&mut self, path: &Path, token: u64) -> Result<(), tonic::Status> {
        let request = PlaceStoreRequest { path: name(path), token };
        self.runtime.block_on(self.client.place_store(request)).map(drop)
    }

    pub fn make_public(&mut self, path: &Path) -> Result<(), tonic::Status> {
        let request = MakePublicRequest { path: name(path) };
        self.runtime.block_on(self.client.make_public(request)).map(drop)
    }

    pub fn sync(&mut self, path: &Path) -> Result<(), tonic::Status> {
        let request = SyncStoreRequest { path: name(path) };
        self.runtime.block_on(self.client.sync_store(request)).map(drop)
    }

    pub fn delete(&mut self, path: &Path) -> Result<(), tonic::Status> {
        let request = DeleteStoreRequest { path: name(path) };
        self.runtime.block_on(self.client.delete_store(request)).map(drop)
    }

    pub fn import(&mut self, path: &Path, image: &Path) -> Result<u64, tonic::Status> {
        let request = ImportImageRequest {
            path: name(path),
            capacity: CAPACITY,
            image: name(image),
            reference: String::new(),
        };
        Ok(self.runtime.block_on(self.client.import_image(request))?.into_inner().branch)
    }

    /// Grants `tenant`, whose stores are in `dir`, its socket, given to `uid`, if named.
    pub fn grant(
        &mut self,
        tenant: &str,
        dir: &Path,
        uid: Option<u32>,
    ) -> Result<PathBuf, tonic::Status> {
        let request = GrantRequest { tenant: tenant.to_owned(), dir: name(dir), uid, gid: None };
        Ok(self.runtime.block_on(self.client.grant(request))?.into_inner().socket.into())
    }

    pub fn revoke(&mut self, tenant: &str) -> Result<(), tonic::Status> {
        let request = RevokeRequest { tenant: tenant.to_owned() };
        self.runtime.block_on(self.client.revoke(request)).map(drop)
    }

    pub fn stats(&mut self) -> GetStatsResponse {
        self.runtime.block_on(self.client.get_stats(GetStatsRequest {})).unwrap().into_inner()
    }
}

pub const CAPACITY: u64 = 1 << 26;

pub fn name(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

/// The code and errno of a call that failed.
pub fn failure<T: std::fmt::Debug>(result: Result<T, tonic::Status>) -> (Code, Errno) {
    let status = result.expect_err("the call succeeded");
    (status.code(), errno(&status).expect("the status carries an errno"))
}

/// Writes `bytes` as the first extent of `store` and commits, setting its root to `root`.
pub fn write(store: &RemoteExtentStore, bytes: &[u8], root: &[u8]) {
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    store.write(extent, 0, &[VolatileSlice::from(bytes)]).unwrap();
    store.commit(Some(root)).unwrap();
}
