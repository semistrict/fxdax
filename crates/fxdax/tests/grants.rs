// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-43.4: tenants granted VM sockets of their own through the control API. A connection's
//! tenant is the socket it came in on: its VM process names stores in the tenant's directory
//! alone. A revoked tenant's socket takes no more connections. A tenant's socket works
//! bind-mounted into a jail, as the host's VM manager mounts it. TASK-154: a tenant reaches of the
//! bucket what the VM manager puts in its directory.

#![cfg(target_os = "linux")]

mod common;

use common::daemon::{CAPACITY, Daemon, failure, write};
use common::{cache, copies, pattern};
use fxdax::{Errno, ExtentId, RemoteExtentStore, VolatileSliceMut};
use std::path::{Path, PathBuf};
use tonic::Code;

/// The errno a VM process connecting to `socket` to open the store at `path` is refused with.
fn refused(socket: &Path, path: &Path) -> Option<Errno> {
    RemoteExtentStore::connect(socket, path, CAPACITY).err().and_then(|err| err.errno())
}

/// Each tenant's VM processes open stores in its directory, and are refused another's, by
/// whatever path; granting again returns the socket.
#[test]
fn tenants_name_stores_in_their_directory_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let (acme, globex) = (dir.path().join("acme"), dir.path().join("globex"));
    std::fs::create_dir(&acme).unwrap();
    std::fs::create_dir(&globex).unwrap();
    let ours = daemon.grant("acme", &acme, None).unwrap();
    assert_eq!(ours, dir.path().join("tenants/acme.sock"));
    let theirs = daemon.grant("globex", &globex, None).unwrap();

    let vm = RemoteExtentStore::connect(&ours, acme.join("vm"), CAPACITY).unwrap();
    write(&vm, &pattern(1), b"acme");
    let their_vm = RemoteExtentStore::connect(&theirs, globex.join("vm"), CAPACITY).unwrap();
    std::os::unix::fs::symlink(&globex, acme.join("link")).unwrap();
    for path in [
        globex.join("vm"),
        globex.join("new"),
        acme.join("../globex/new"),
        acme.join("link/new"),
        acme.clone(),
        dir.path().join("outside"),
    ] {
        assert_eq!(refused(&ours, &path), Some(Errno::EACCES), "{}", path.display());
    }
    // A snapshot's point to restore from, another tenant's.
    let restored = RemoteExtentStore::connect_forked(
        &ours,
        acme.join("restored"),
        globex.join("vm"),
        their_vm.branch(),
        CAPACITY,
    );
    assert_eq!(restored.err().and_then(|err| err.errno()), Some(Errno::EACCES));

    assert_eq!(daemon.grant("acme", &acme, None).unwrap(), ours);
    assert_eq!(failure(daemon.grant("acme", &globex, None)), (Code::AlreadyExists, Errno::EEXIST));
    assert_eq!(
        failure(daemon.grant("../acme", &acme, None)),
        (Code::InvalidArgument, Errno::EINVAL)
    );
    let none = dir.path().join("none");
    assert_eq!(failure(daemon.grant("initech", &none, None)), (Code::NotFound, Errno::ENOENT));
}

/// A revoked tenant's socket goes, and takes no more connections, while its VM processes
/// connected keep their stores; granted again, its VM processes open its stores again.
#[test]
fn revoked_tenants_take_no_new_connections() {
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let acme = dir.path().join("acme");
    std::fs::create_dir(&acme).unwrap();
    let socket = daemon.grant("acme", &acme, None).unwrap();
    let vm = RemoteExtentStore::connect(&socket, acme.join("vm"), CAPACITY).unwrap();
    write(&vm, &pattern(2), b"before");

    daemon.revoke("acme").unwrap();
    assert!(!socket.exists(), "the socket is there");
    let connected = std::os::unix::net::UnixStream::connect(&socket);
    assert_eq!(connected.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    vm.commit(Some(b"after")).unwrap();
    assert_eq!(vm.root().unwrap(), b"after");
    assert_eq!(failure(daemon.revoke("acme")), (Code::NotFound, Errno::ENOENT));
    drop(vm);

    let socket = daemon.grant("acme", &acme, None).unwrap();
    let vm = RemoteExtentStore::connect(&socket, acme.join("vm"), CAPACITY).unwrap();
    assert_eq!(vm.root().unwrap(), b"after");
}

/// TASK-79.36: a tenant revoked whose VM processes live on, granted again, is the tenant they are
/// of: the VM processes it connects share its arena with them, and with it the pages of the stores
/// they have open, which they follow.
#[test]
fn tenants_granted_again_while_alive_go_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let acme = dir.path().join("acme");
    std::fs::create_dir(&acme).unwrap();
    let socket = daemon.grant("acme", &acme, None).unwrap();
    let vm = RemoteExtentStore::connect(&socket, acme.join("vm"), CAPACITY).unwrap();
    let bytes = pattern(4);
    cache(&vm, &bytes);

    daemon.revoke("acme").unwrap();
    let socket = daemon.grant("acme", &acme, None).unwrap();
    let follower = RemoteExtentStore::connect_read_only(&socket, acme.join("vm")).unwrap();
    assert_eq!(copies(follower.physical_memory(), &bytes), 1, "the VM's pages, in its memory");
}

/// The environment of this test's process run in a jail ([`jailed`]): the socket's path there,
/// the tenant's socket's path on the host, and the store to open.
const JAIL_SOCKET: &str = "FXDAX_TEST_JAIL_SOCKET";
const HOST_SOCKET: &str = "FXDAX_TEST_HOST_SOCKET";
const JAIL_STORE: &str = "FXDAX_TEST_JAIL_STORE";

/// A tenant's socket, given to its jails' user and bind-mounted into a jail, as the host's VM
/// manager does with Firecracker's jailer: in a mount namespace of its own, where the tenants'
/// sockets' directory is hidden, a VM process of that user opens the tenant's store through it.
/// Needs `sudo -n`, as Lima's VMs have it.
#[test]
fn tenant_sockets_work_bind_mounted_into_jails() {
    if let Some(socket) = std::env::var_os(JAIL_SOCKET) {
        return jailed(socket.as_ref());
    }
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let acme = dir.path().join("acme");
    std::fs::create_dir(&acme).unwrap();
    // SAFETY: Plain system calls.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let socket = daemon.grant("acme", &acme, Some(uid)).unwrap();
    let jail = dir.path().join("jail");
    std::fs::create_dir(&jail).unwrap();
    let jail_socket = jail.join("extentd.sock");
    std::fs::write(&jail_socket, b"").unwrap();
    let store = acme.join("vm");

    let script = r#"mount --bind "$1" "$2" && mount -t tmpfs none "$3" &&
        exec setpriv --reuid "$4" --regid "$5" --clear-groups \
            env "$6" "$7" "$8" "$9" --exact "${10}" --nocapture"#;
    let test = "tenant_sockets_work_bind_mounted_into_jails";
    let environment = [
        format!("{JAIL_SOCKET}={}", jail_socket.display()),
        format!("{HOST_SOCKET}={}", socket.display()),
        format!("{JAIL_STORE}={}", store.display()),
    ];
    let status = std::process::Command::new("sudo")
        .args(["-n", "unshare", "--mount", "--propagation", "private", "sh", "-c", script, "sh"])
        .arg(&socket)
        .arg(&jail_socket)
        .arg(socket.parent().unwrap())
        .args([uid.to_string(), gid.to_string()])
        .args(environment)
        .arg(std::env::current_exe().unwrap())
        .arg(test)
        .status()
        .unwrap();
    assert!(status.success(), "the jailed VM process failed: {status}");
    assert_eq!(daemon.stat(&store).unwrap().used, 64 << 10);
}

/// The VM process in the jail: the tenants' sockets hidden, it opens its store through the one
/// bind-mounted at `socket`, and writes it.
fn jailed(socket: &Path) {
    let host_socket = std::env::var_os(HOST_SOCKET).unwrap();
    assert!(!Path::new(&host_socket).exists(), "the tenants' sockets are not hidden");
    let store = std::env::var_os(JAIL_STORE).unwrap();
    let vm = RemoteExtentStore::connect(socket, store, CAPACITY).unwrap();
    write(&vm, &pattern(3), b"jailed");
}

/// A host of two over one bucket: a daemon of its own, and the tenant acme's directory and socket
/// on it. The two are threads of one process, as a bucket in memory is one process's: they share
/// their extent cache, which [`Host::forget_what_it_read`] empties, and the bucket alone.
struct Host {
    daemon: Daemon,
    acme: PathBuf,
    socket: PathBuf,
}

impl Host {
    fn new(dir: &Path, name: &str) -> Self {
        let dir = dir.join(name);
        let acme = dir.join("acme");
        std::fs::create_dir_all(&acme).unwrap();
        let mut daemon = Daemon::new(&dir);
        let socket = daemon.grant(&format!("acme-{name}"), &acme, None).unwrap();
        Self { daemon, acme, socket }
    }

    /// What the host read goes from memory, as it never had it on another host.
    fn forget_what_it_read(&mut self) {
        self.daemon.shrink_memory(None, 0).unwrap();
    }

    /// A snapshot of a VM of the tenant, as Firecracker takes it, in the bucket.
    fn snapshot(&mut self) -> Snapshot {
        let vm = RemoteExtentStore::connect(&self.socket, self.acme.join("vm"), CAPACITY).unwrap();
        write(&vm, &pattern(5), b"snapshotted");
        let extent = ExtentId { branch: vm.branch(), sequence: 1 };
        let point = self.acme.join("point");
        vm.fork_quiescent(&point, vm.branch() + 1).unwrap();
        drop(vm);
        self.daemon.sync(&point).unwrap();
        // As the VM manager reads it: not the snapshot's word for it, which a VM process wrote.
        Snapshot { branch: self.daemon.stat(&point).unwrap().branch, extent }
    }
}

/// What the VM manager keeps of a snapshot: its point's branch. And the extent the VM wrote.
struct Snapshot {
    branch: u64,
    extent: ExtentId,
}

/// A VM process's restore through `socket`, into a store of its own at `store`, of the snapshot
/// whose point it names `point`, of `branch`: the request Firecracker's restore makes.
fn restore(
    socket: &Path,
    store: &Path,
    point: &Path,
    branch: u64,
) -> Result<RemoteExtentStore, Option<Errno>> {
    RemoteExtentStore::connect_forked(socket, store, point, branch, CAPACITY)
        .map_err(|err| err.errno())
}

/// The bytes of the VM's `extent`, as the snapshot holds them, read through `store`.
fn snapshotted(store: &RemoteExtentStore, extent: ExtentId) -> bool {
    let mut read = vec![0; 64 << 10];
    store.read(extent, 0, &mut [VolatileSliceMut::from(&mut read[..])]).unwrap();
    read == pattern(5)
}

/// TASK-154: a tenant's snapshot restores on another host, which has nothing of it, through the
/// request every restore makes: refused until the VM manager makes the point in the tenant's
/// directory, then served, what it reads coming from the bucket, as many restores as there are.
#[test]
fn tenants_restore_on_another_host_what_their_vm_manager_took() {
    let dir = tempfile::tempdir().unwrap();
    let (mut here, mut there) = (Host::new(dir.path(), "here"), Host::new(dir.path(), "there"));
    let snapshot = here.snapshot();
    here.forget_what_it_read();
    let fetched = there.daemon.stats().fetched_bytes;

    let (point, vm) = (there.acme.join("point"), there.acme.join("vm"));
    let refused = restore(&there.socket, &vm, &point, snapshot.branch);
    assert_eq!(refused.map(drop), Err(Some(Errno::ENOENT)));
    assert!(!point.exists(), "a point was taken for the branch a VM process named");

    assert_eq!(there.daemon.create(&point, snapshot.branch, None).unwrap(), snapshot.branch);
    let restored = restore(&there.socket, &vm, &point, snapshot.branch).unwrap();
    assert_ne!(restored.branch(), snapshot.branch);
    assert_eq!(restored.root().unwrap(), b"snapshotted");
    assert!(snapshotted(&restored, snapshot.extent), "the restored VM reads other bytes");
    let read = there.daemon.stats().fetched_bytes - fetched;
    assert!(read >= 64 << 10, "{read} bytes came from the bucket");
    write(&restored, &pattern(6), b"restored");

    // A zygote's other child, whatever it names, and the first's writes none of its.
    let other = restore(&there.socket, &there.acme.join("other"), &point, 0).unwrap();
    assert_eq!(other.root().unwrap(), b"snapshotted");
    assert_eq!(restored.root().unwrap(), b"restored");
}

/// TASK-154: the VM manager's taking a branch for a tenant gives another tenant nothing: on one
/// host, the same request, naming that branch and a point in its own directory, is refused, and
/// its naming the first tenant's point too.
#[test]
fn tenants_take_no_branch_given_to_another() {
    let dir = tempfile::tempdir().unwrap();
    let (mut here, mut there) = (Host::new(dir.path(), "here"), Host::new(dir.path(), "there"));
    let snapshot = here.snapshot();
    let given = there.acme.join("point");
    there.daemon.create(&given, snapshot.branch, None).unwrap();

    let globex = dir.path().join("there/globex");
    std::fs::create_dir(&globex).unwrap();
    let socket = there.daemon.grant("globex", &globex, None).unwrap();
    let (vm, point) = (globex.join("vm"), globex.join("point"));
    let refused = restore(&socket, &vm, &point, snapshot.branch);
    assert_eq!(refused.map(drop), Err(Some(Errno::ENOENT)));
    assert!(!point.exists(), "a point was taken for the branch a VM process named");
    let refused = restore(&socket, &vm, &given, snapshot.branch);
    assert_eq!(refused.map(drop), Err(Some(Errno::EACCES)));

    let restored = restore(&there.socket, &there.acme.join("vm"), &given, 0).unwrap();
    assert_eq!(restored.root().unwrap(), b"snapshotted");
}

/// TASK-154: what the VM manager reads of a point, the branch to give a tenant elsewhere, a VM
/// process can't make another tenant's: a fork names its child, and another branch's ID fails,
/// leaving no point to read.
#[test]
fn vm_processes_make_no_point_of_another_branch() {
    let dir = tempfile::tempdir().unwrap();
    let mut here = Host::new(dir.path(), "here");
    let vm = RemoteExtentStore::connect(&here.socket, here.acme.join("vm"), CAPACITY).unwrap();
    write(&vm, &pattern(1), b"ours");
    let theirs = here.daemon.open(&dir.path().join("theirs"), CAPACITY);
    write(&theirs, &pattern(2), b"theirs");

    let point = here.acme.join("point");
    assert_eq!(vm.fork_quiescent(&point, theirs.branch()), Err(Errno::EEXIST));
    assert_eq!(failure(here.daemon.stat(&point)), (Code::NotFound, Errno::ENOENT));
}

/// TASK-154: DropStore ends what CreateStore gave: the same request is refused, before any restore
/// and after, those restored going on; the branch stays, which DeleteStore would have deleted, so
/// that a host takes it again.
#[test]
fn points_dropped_restore_no_more_and_leave_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let (mut here, mut there) = (Host::new(dir.path(), "here"), Host::new(dir.path(), "there"));
    let snapshot = here.snapshot();
    let (socket, point, vm) = (&there.socket, there.acme.join("point"), there.acme.join("vm"));

    there.daemon.create(&point, snapshot.branch, None).unwrap();
    there.daemon.drop_store(&point).unwrap();
    assert!(!point.exists(), "the copy stayed");
    let refused = restore(socket, &vm, &point, snapshot.branch);
    assert_eq!(refused.map(drop), Err(Some(Errno::ENOENT)));

    there.daemon.create(&point, snapshot.branch, None).unwrap();
    let restored = restore(socket, &vm, &point, snapshot.branch).unwrap();
    there.daemon.drop_store(&point).unwrap();
    let other = there.acme.join("other");
    let refused = restore(socket, &other, &point, snapshot.branch);
    assert_eq!(refused.map(drop), Err(Some(Errno::EIO)));
    assert!(snapshotted(&restored, snapshot.extent), "what was restored went with the point");
    write(&restored, &pattern(6), b"restored");

    // At another path while forks hold that one's directory.
    let again = there.acme.join("point-again");
    there.daemon.create(&again, snapshot.branch, None).unwrap();
    assert_eq!(restore(socket, &other, &again, 0).unwrap().root().unwrap(), b"snapshotted");
}

/// TASK-154: a host's copy of a point is a writer of its branch, and a restore's fork a commit
/// there (doc-7, Forks): once one host forks it, the others' copies are stale, the snapshot's own
/// host's too, and refuse restores (EIO), until the VM manager takes the branch anew.
#[test]
fn copies_of_a_point_another_host_forked_are_taken_anew() {
    let dir = tempfile::tempdir().unwrap();
    let (mut here, mut there) = (Host::new(dir.path(), "here"), Host::new(dir.path(), "there"));
    let snapshot = here.snapshot();
    let point = there.acme.join("point");
    there.daemon.create(&point, snapshot.branch, None).unwrap();
    let _restored = restore(&there.socket, &there.acme.join("vm"), &point, 0).unwrap();

    let (stale, vm) = (here.acme.join("point"), here.acme.join("restored"));
    assert_eq!(restore(&here.socket, &vm, &stale, 0).map(drop), Err(Some(Errno::EIO)));
    here.daemon.drop_store(&stale).unwrap();
    here.daemon.create(&stale, snapshot.branch, None).unwrap();
    assert_eq!(restore(&here.socket, &vm, &stale, 0).unwrap().root().unwrap(), b"snapshotted");
}

/// TASK-154: a point whose branch's last writer is of a later placement than this host's is taken
/// placed: a host's own token is refused (ESTALE), as is every restore of it, and CreateStore
/// names the placement, which no request of a VM process's could.
#[test]
fn points_of_later_placements_are_taken_placed() {
    let dir = tempfile::tempdir().unwrap();
    let (mut here, mut there) = (Host::new(dir.path(), "here"), Host::new(dir.path(), "there"));
    let snapshot = here.snapshot();
    let later = 7;
    here.daemon.place(&here.acme.join("point"), later).unwrap();
    here.daemon.sync(&here.acme.join("point")).unwrap();

    let (point, vm) = (there.acme.join("point"), there.acme.join("vm"));
    there.daemon.create(&point, snapshot.branch, None).unwrap();
    let refused = restore(&there.socket, &vm, &point, snapshot.branch);
    assert_eq!(refused.map(drop), Err(Some(Errno::ESTALE)));
    there.daemon.drop_store(&point).unwrap();

    there.daemon.create(&point, snapshot.branch, Some(later)).unwrap();
    let restored = restore(&there.socket, &vm, &point, snapshot.branch).unwrap();
    assert_eq!(restored.root().unwrap(), b"snapshotted");
}
