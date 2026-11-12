// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-136.5: forks attached to stores through the control API: a VM reads and maps what they
//! hold, read-only, through its one device, as a stopped store's next VM does, and a fork's, and a
//! tenant reaches no other tenant's.

#![cfg(target_os = "linux")]

mod common;

use common::daemon::{CAPACITY, Daemon, failure, name, write};
use common::{copies, pattern};
use fxdax::extentd::control::proto::{AttachBaseRequest, MakePublicRequest};
use fxdax::{Errno, ExtentId, RemoteExtentStore, VolatileSliceMut};
use std::path::{Path, PathBuf};
use tonic::Code;

fn attach(daemon: &mut Daemon, store: &Path, base: &Path) -> Result<(), tonic::Status> {
    let request = AttachBaseRequest { store: name(store), base: name(base) };
    daemon.runtime.block_on(daemon.client.attach_base(request)).map(drop)
}

fn make_generation_public(daemon: &mut Daemon, path: &Path) -> Result<(), tonic::Status> {
    let request = MakePublicRequest { path: name(path), generation: true };
    daemon.runtime.block_on(daemon.client.make_public(request)).map(drop)
}

fn read(store: &RemoteExtentStore, extent: ExtentId) -> Result<Vec<u8>, Errno> {
    let mut data = pattern(0);
    data.fill(0);
    store.read(extent, 0, &mut [VolatileSliceMut::from(&mut data[..])])?;
    Ok(data)
}

/// A VM's store at `dir`/publisher holding `pattern(seed)` in its first extent, and the generation
/// forked from it at `dir`/`generation`, which outlasts the VM: the extent, and the fork's path.
fn publish(daemon: &mut Daemon, dir: &Path, seed: u64, generation: &str) -> (ExtentId, PathBuf) {
    let (publisher, fork) = (dir.join(format!("publisher-{generation}")), dir.join(generation));
    let vm = daemon.open(&publisher, CAPACITY);
    let extent = ExtentId { branch: vm.branch(), sequence: 1 };
    write(&vm, &pattern(seed), b"published");
    daemon.fork(&publisher, &fork).unwrap();
    (extent, fork)
}

/// A VM reads what is attached to its store as it runs, and nothing before, nor of what isn't.
#[test]
fn vms_read_what_is_attached_as_they_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let (extent, base) = publish(&mut daemon, dir.path(), 1, "base");
    let (other_extent, _other) = publish(&mut daemon, dir.path(), 2, "other");
    let sandbox = dir.path().join("sandbox");
    let vm = daemon.open(&sandbox, CAPACITY);
    assert_eq!(read(&vm, extent), Err(Errno::ENOENT));

    attach(&mut daemon, &sandbox, &base).unwrap();
    assert!(read(&vm, extent).unwrap() == pattern(1), "the VM reads other bytes");
    assert_eq!(read(&vm, other_extent), Err(Errno::ENOENT));
}

/// What is attached to a store stopped is there for its next VM, as for the VM after, for its
/// forks' and for a VM restored from a point of it.
#[test]
fn stores_keep_what_is_attached() {
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let (extent, base) = publish(&mut daemon, dir.path(), 3, "base");
    let (sandbox, fork) = (dir.path().join("sandbox"), dir.path().join("fork"));
    let vm = daemon.open(&sandbox, CAPACITY);
    write(&vm, &pattern(4), b"sandbox");
    drop(vm);
    daemon.sync(&sandbox).unwrap();

    attach(&mut daemon, &sandbox, &base).unwrap();
    let vm = daemon.open(&sandbox, CAPACITY);
    assert!(read(&vm, extent).unwrap() == pattern(3), "the VM reads other bytes");
    drop(vm);
    daemon.sync(&sandbox).unwrap();
    let vm = daemon.open(&sandbox, CAPACITY);
    assert!(read(&vm, extent).unwrap() == pattern(3), "the VM after reads other bytes");

    daemon.fork(&sandbox, &fork).unwrap();
    let forked = daemon.open(&fork, CAPACITY);
    assert!(read(&forked, extent).unwrap() == pattern(3), "the fork's VM reads other bytes");
    drop(forked);

    let restored = dir.path().join("restored");
    let branch = vm.branch();
    drop(vm);
    daemon.sync(&sandbox).unwrap();
    let vm_restored =
        RemoteExtentStore::connect_forked(&daemon.vms, &restored, &sandbox, branch, CAPACITY)
            .unwrap();
    assert!(read(&vm_restored, extent).unwrap() == pattern(3), "the restored VM reads other bytes");
}

/// A store that isn't public attaches only to stores of its tenant, and a public generation to
/// any, whose VMs share its pages, of the default arena, once.
#[test]
fn tenants_reach_their_own_bases_and_public_ones() {
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let (acme, globex) = (dir.path().join("acme"), dir.path().join("globex"));
    std::fs::create_dir(&acme).unwrap();
    std::fs::create_dir(&globex).unwrap();
    let acme_socket = daemon.grant("acme", 1, &acme, None).unwrap();
    let globex_socket = daemon.grant("globex", 2, &globex, None).unwrap();
    let open =
        |socket: &Path, path: PathBuf| RemoteExtentStore::connect(socket, path, CAPACITY).unwrap();

    // A private generation of acme's.
    let vm = open(&acme_socket, acme.join("publisher"));
    let private = ExtentId { branch: vm.branch(), sequence: 1 };
    write(&vm, &pattern(5), b"private");
    let generation = acme.join("generation");
    daemon.fork(&acme.join("publisher"), &generation).unwrap();

    let (acme_vm, globex_vm) =
        (open(&acme_socket, acme.join("vm")), open(&globex_socket, globex.join("vm")));
    let refused = attach(&mut daemon, &globex.join("vm"), &generation);
    assert_eq!(failure(refused), (Code::PermissionDenied, Errno::EACCES));
    assert_eq!(read(&globex_vm, private), Err(Errno::ENOENT));
    attach(&mut daemon, &acme.join("vm"), &generation).unwrap();
    assert!(read(&acme_vm, private).unwrap() == pattern(5), "acme reads other bytes");

    // A public one, of a store of no tenant's that goes on being written.
    let (public, base) = publish(&mut daemon, dir.path(), 6, "public");
    make_generation_public(&mut daemon, &base).unwrap();
    assert!(daemon.stat(&base).unwrap().public);
    for (store, vm) in [(acme.join("vm"), &acme_vm), (globex.join("vm"), &globex_vm)] {
        attach(&mut daemon, &store, &base).unwrap();
        assert!(read(vm, public).unwrap() == pattern(6), "a tenant reads other bytes");
    }
    let publisher = dir.path().join("publisher-public");
    let writer = daemon.open(&publisher, CAPACITY);
    let next = ExtentId { branch: writer.branch(), sequence: 2 };
    writer.write(next, 0, &[fxdax::VolatileSlice::from(&pattern(7)[..])]).unwrap();
    writer.commit(None).unwrap();
    assert_eq!(copies(fxdax::pages::physical_memory(), &pattern(6)), 1, "of one copy");
    assert_eq!(copies(acme_vm.physical_memory(), &pattern(6)), 0, "in acme's arena");
    assert_eq!(copies(globex_vm.physical_memory(), &pattern(6)), 0, "in globex's arena");
}

/// A generation goes as a store does, once no VM reads through to it, and stays until then
/// whatever its store does.
#[test]
fn generations_go_once_nothing_reads_them() {
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let (extent, base) = publish(&mut daemon, dir.path(), 8, "base");
    let publisher = dir.path().join("publisher-base");
    let moved_on = daemon.open(&publisher, CAPACITY);
    moved_on.free(extent).unwrap();
    moved_on.commit(None).unwrap();
    drop(moved_on);
    daemon.sync(&publisher).unwrap();

    let sandbox = dir.path().join("sandbox");
    let vm = daemon.open(&sandbox, CAPACITY);
    attach(&mut daemon, &sandbox, &base).unwrap();
    assert!(read(&vm, extent).unwrap() == pattern(8), "the VM reads other bytes");
    assert_eq!(failure(daemon.delete(&base)), (Code::FailedPrecondition, Errno::EBUSY));

    drop(vm);
    daemon.sync(&sandbox).unwrap();
    daemon.delete(&base).unwrap();
    // The store it was attached to says so as it reads, and opens all the same.
    let vm = daemon.open(&sandbox, CAPACITY);
    assert_eq!(read(&vm, extent), Err(Errno::ENOENT));
}
