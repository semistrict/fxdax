// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-43.4: tenants granted VM sockets of their own through the control API. A connection's
//! tenant is the socket it came in on: its VM process names stores in the tenant's directory
//! alone. A revoked tenant's socket takes no more connections. A tenant's socket works
//! bind-mounted into a jail, as the host's VM manager mounts it. TASK-154: a tenant reaches of the
//! bucket what the VM manager puts in its directory. TASK-155: the daemon's own socket is its
//! user's alone, mounted or not.

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
        // In the bucket once taken, with no sync. As the VM manager reads it: not the snapshot's
        // word for it, which a VM process wrote.
        let stat = self.daemon.stat(&point).unwrap();
        let cell = fxdax::Cell { tenant: stat.tenant, id: stat.cell };
        Snapshot { branch: fxdax::Branch { cell, id: stat.branch }, extent }
    }
}

/// What the VM manager keeps of a snapshot: its point's branch, in its cell (TASK-364). And the
/// extent the VM wrote.
struct Snapshot {
    branch: fxdax::Branch,
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
    read(store, extent) == pattern(5)
}

/// What `store` holds of `extent`, a block.
fn read(store: &RemoteExtentStore, extent: ExtentId) -> Vec<u8> {
    let mut read = vec![0; 64 << 10];
    store.read(extent, 0, &mut [VolatileSliceMut::from(&mut read[..])]).unwrap();
    read
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
    let refused = restore(&there.socket, &vm, &point, snapshot.branch.id);
    assert_eq!(refused.map(drop), Err(Some(Errno::ENOENT)));
    assert!(!point.exists(), "a point was taken for the branch a VM process named");

    assert_eq!(there.daemon.create(&point, snapshot.branch, None).unwrap(), snapshot.branch);
    let restored = restore(&there.socket, &vm, &point, snapshot.branch.id).unwrap();
    assert_ne!(restored.branch(), snapshot.branch.id);
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
    let refused = restore(&socket, &vm, &point, snapshot.branch.id);
    assert_eq!(refused.map(drop), Err(Some(Errno::ENOENT)));
    assert!(!point.exists(), "a point was taken for the branch a VM process named");
    let refused = restore(&socket, &vm, &given, snapshot.branch.id);
    assert_eq!(refused.map(drop), Err(Some(Errno::EACCES)));

    let restored = restore(&there.socket, &there.acme.join("vm"), &given, 0).unwrap();
    assert_eq!(restored.root().unwrap(), b"snapshotted");
}

/// TASK-154: what the VM manager reads of a point, the branch to give a tenant elsewhere, a VM
/// process can't make another tenant's: a fork names its child's ID, and a branch is its cell's
/// and its ID (TASK-364), so a child of another branch's ID is a branch of its own, in a new cell,
/// holding what the VM's store held, which the VM manager reads as the point's.
#[test]
fn vm_processes_make_no_point_of_another_branch() {
    let dir = tempfile::tempdir().unwrap();
    let mut here = Host::new(dir.path(), "here");
    let vm = RemoteExtentStore::connect(&here.socket, here.acme.join("vm"), CAPACITY).unwrap();
    write(&vm, &pattern(1), b"ours");
    let theirs_path = dir.path().join("theirs");
    let theirs = here.daemon.open(&theirs_path, CAPACITY);
    write(&theirs, &pattern(2), b"theirs");

    let point = here.acme.join("point");
    assert_eq!(vm.fork_quiescent(&point, theirs.branch()), Ok(()));
    let (made, theirs) = (here.daemon.branch(&point).unwrap(), here.daemon.branch(&theirs_path));
    let theirs = theirs.unwrap();
    assert_eq!(made.id, theirs.id);
    assert_ne!(made.cell, theirs.cell, "the point is a branch of a cell of its own");
    let restored = restore(&here.socket, &here.acme.join("restored"), &point, 0).unwrap();
    assert_eq!(restored.root().unwrap(), b"ours");
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
    let refused = restore(socket, &vm, &point, snapshot.branch.id);
    assert_eq!(refused.map(drop), Err(Some(Errno::ENOENT)));

    there.daemon.create(&point, snapshot.branch, None).unwrap();
    let restored = restore(socket, &vm, &point, snapshot.branch.id).unwrap();
    there.daemon.drop_store(&point).unwrap();
    // Gone, though forks of it live, which hold none of its directory (TASK-165.7).
    assert!(!point.exists(), "the copy stayed");
    let other = there.acme.join("other");
    let refused = restore(socket, &other, &point, snapshot.branch.id);
    assert_eq!(refused.map(drop), Err(Some(Errno::ENOENT)));
    assert!(snapshotted(&restored, snapshot.extent), "what was restored went with the point");
    write(&restored, &pattern(6), b"restored");

    // At another path.
    let again = there.acme.join("point-again");
    there.daemon.create(&again, snapshot.branch, None).unwrap();
    assert_eq!(restore(socket, &other, &again, 0).unwrap().root().unwrap(), b"snapshotted");
}

/// TASK-158: a point is read by any number of hosts and written by none: copies of it on hosts
/// that forked it, in any order, restore on, the snapshot's own host's too, and none went stale.
/// (It pinned the limit this removed, as `copies_of_a_point_another_host_forked_are_taken_anew`:
/// the other hosts' copies failing with EIO until DropStore and CreateStore.)
#[test]
fn copies_of_a_point_another_host_forked_restore_on() {
    let dir = tempfile::tempdir().unwrap();
    let (mut here, mut there) = (Host::new(dir.path(), "here"), Host::new(dir.path(), "there"));
    let snapshot = here.snapshot();
    let point = there.acme.join("point");
    there.daemon.create(&point, snapshot.branch, None).unwrap();
    let restored = restore(&there.socket, &there.acme.join("vm"), &point, 0).unwrap();

    let (own, vm) = (here.acme.join("point"), here.acme.join("restored"));
    let again = restore(&here.socket, &vm, &own, 0).unwrap();
    assert_eq!(again.root().unwrap(), b"snapshotted");
    let other = restore(&there.socket, &there.acme.join("other"), &point, 0).unwrap();
    assert_eq!(other.root().unwrap(), b"snapshotted");
    assert!(snapshotted(&restored, snapshot.extent), "the first restore lost what it holds");
}

/// TASK-158: one snapshot restored on several hosts at once: the requests of each race, on threads
/// of their own, and each host's VMs read the snapshot, and write on their own.
#[test]
fn several_hosts_restore_one_snapshot_at_once() {
    const HOSTS: usize = 3;
    const RESTORES: usize = 4;
    let dir = tempfile::tempdir().unwrap();
    let mut hosts: Vec<_> =
        (0..HOSTS).map(|host| Host::new(dir.path(), &format!("host{host}"))).collect();
    let snapshot = hosts[0].snapshot();
    for host in &mut hosts[1..] {
        host.daemon.create(&host.acme.join("point"), snapshot.branch, None).unwrap();
    }
    let restored: Vec<_> = std::thread::scope(|scope| {
        let hosts = &hosts;
        let restores: Vec<_> = (0..HOSTS * RESTORES)
            .map(|at| {
                let host = &hosts[at % HOSTS];
                let vm = host.acme.join(format!("restored{at}"));
                scope.spawn(move || restore(&host.socket, &vm, &host.acme.join("point"), 0))
            })
            .collect();
        restores.into_iter().map(|restore| restore.join().unwrap().unwrap()).collect()
    });
    let mut branches: Vec<_> = restored.iter().map(RemoteExtentStore::branch).collect();
    branches.sort();
    branches.dedup();
    assert_eq!(branches.len(), HOSTS * RESTORES, "two restores of one branch");
    for (at, vm) in restored.iter().enumerate() {
        assert_eq!(vm.root().unwrap(), b"snapshotted", "restore {at}");
        assert!(snapshotted(vm, snapshot.extent), "restore {at} reads other bytes");
        write(vm, &pattern(at as u64), format!("restored {at}").as_bytes());
    }
    for (at, vm) in restored.iter().enumerate() {
        assert_eq!(vm.root().unwrap(), format!("restored {at}").as_bytes());
    }
}

/// TASK-158: a point has no writer: a VM process opening one to write it, on its snapshot's host
/// or another, is refused, and the snapshot stays as it was. Before, it could write it.
#[test]
fn vm_processes_write_no_point() {
    let dir = tempfile::tempdir().unwrap();
    let (mut here, mut there) = (Host::new(dir.path(), "here"), Host::new(dir.path(), "there"));
    let snapshot = here.snapshot();
    let point = there.acme.join("point");
    there.daemon.create(&point, snapshot.branch, None).unwrap();
    for (socket, point) in [(&here.socket, &here.acme.join("point")), (&there.socket, &point)] {
        assert_eq!(refused(socket, point), Some(Errno::EROFS));
        let restored = restore(socket, &point.with_file_name("vm-after"), point, 0).unwrap();
        assert_eq!(restored.root().unwrap(), b"snapshotted");
    }
}

/// TASK-160.10: a point's capacity, as all else of it, changes on no host: ResizeStore of one is
/// refused (`EROFS`), closed on its snapshot's host, kept open for restores on another, and the
/// restores after it have the capacity it had. Before, the daemon opened it to resize it without
/// its remote, which says it is a point, and grew it.
#[test]
fn points_are_resized_nowhere() {
    let dir = tempfile::tempdir().unwrap();
    let (mut here, mut there) = (Host::new(dir.path(), "here"), Host::new(dir.path(), "there"));
    let snapshot = here.snapshot();
    let point = there.acme.join("point");
    there.daemon.create(&point, snapshot.branch, None).unwrap();
    drop(restore(&there.socket, &there.acme.join("vm"), &point, 0).unwrap());
    let (refused, own) = ((Code::FailedPrecondition, Errno::EROFS), here.acme.join("point"));
    for (host, point) in [(&mut here, own), (&mut there, point)] {
        assert_eq!(failure(host.daemon.resize(&point, 2 * CAPACITY)), refused);
        assert_eq!(host.daemon.stat(&point).unwrap().capacity, CAPACITY);
        let restored = restore(&host.socket, &point.with_file_name("after"), &point, 0).unwrap();
        assert_eq!(restored.info().unwrap().capacity, CAPACITY);
    }
}

/// TASK-158: a point has no lease, so that a host needs no placement to restore it, however late
/// the placement that took it. (It pinned the limit this removed, as
/// `points_of_later_placements_are_taken_placed`: a restore refused with ESTALE until CreateStore
/// named the token.) Placing a copy changes nothing.
#[test]
fn points_restore_whatever_placement_took_them() {
    let dir = tempfile::tempdir().unwrap();
    let (mut here, mut there) = (Host::new(dir.path(), "here"), Host::new(dir.path(), "there"));
    let snapshot = here.snapshot();
    let later = 7;
    here.daemon.place(&here.acme.join("point"), later).unwrap();
    here.daemon.sync(&here.acme.join("point")).unwrap();

    let (point, vm) = (there.acme.join("point"), there.acme.join("vm"));
    there.daemon.create(&point, snapshot.branch, None).unwrap();
    let restored = restore(&there.socket, &vm, &point, snapshot.branch.id).unwrap();
    assert_eq!(restored.root().unwrap(), b"snapshotted");
    let own = restore(&here.socket, &here.acme.join("restored"), &here.acme.join("point"), 0);
    assert_eq!(own.unwrap().root().unwrap(), b"snapshotted");
}

/// TASK-158: a base, as any store, made a point by ForkStore's `point`, forks on several hosts at
/// once, as a snapshot's point restores: every host's copy forks while the others' do, into
/// branches of their own, which read the base and write on their own. The base is its writer's
/// still: what it writes after reaches no fork of the point. No point makes one of its own, and no
/// VM process opens one to write it.
#[test]
fn bases_made_points_fork_on_several_hosts_at_once() {
    const HOSTS: usize = 3;
    const FORKS: usize = 3;
    let dir = tempfile::tempdir().unwrap();
    let mut hosts: Vec<_> =
        (0..HOSTS).map(|host| Host::new(dir.path(), &format!("host{host}"))).collect();
    let base = hosts[0].acme.join("base");
    let vm = hosts[0].daemon.open(&base, CAPACITY);
    write(&vm, &pattern(7), b"base");
    let extent = ExtentId { branch: vm.branch(), sequence: 1 };
    drop(vm);
    let point = hosts[0].acme.join("point");
    let branch = hosts[0].daemon.point(&base, &point).unwrap();
    for host in &mut hosts[1..] {
        host.daemon.create(&host.acme.join("point"), branch, None).unwrap();
    }

    let forks: Vec<(usize, PathBuf, fxdax::Branch)> = std::thread::scope(|scope| {
        let forking: Vec<_> = (hosts.iter_mut().enumerate())
            .map(|(at, host)| {
                scope.spawn(move || {
                    let copy = host.acme.join("point");
                    let fork = |n| {
                        let to = host.acme.join(format!("fork{n}"));
                        let branch = host.daemon.fork(&copy, &to).unwrap();
                        (at, to, branch)
                    };
                    (0..FORKS).map(fork).collect::<Vec<_>>()
                })
            })
            .collect();
        forking.into_iter().flat_map(|forking| forking.join().unwrap()).collect()
    });
    let mut branches: Vec<_> = forks.iter().map(|&(_, _, branch)| branch).collect();
    branches.sort();
    branches.dedup();
    assert_eq!(branches.len(), HOSTS * FORKS, "two forks of one branch");
    let opened: Vec<_> =
        forks.iter().map(|(at, to, _)| hosts[*at].daemon.open(to, CAPACITY)).collect();
    for (n, vm) in opened.iter().enumerate() {
        assert_eq!(vm.root().unwrap(), b"base", "fork {n}");
        assert!(read(vm, extent) == pattern(7), "fork {n} reads other bytes");
        write(vm, &pattern(10 + n as u64), format!("fork {n}").as_bytes());
    }
    for (n, vm) in opened.iter().enumerate() {
        assert_eq!(vm.root().unwrap(), format!("fork {n}").as_bytes());
    }

    let vm = hosts[0].daemon.open(&base, CAPACITY);
    vm.commit(Some(b"base, later")).unwrap();
    drop(vm);
    let other = &mut hosts[1];
    let (copy, later) = (other.acme.join("point"), other.acme.join("later"));
    other.daemon.fork(&copy, &later).unwrap();
    assert_eq!(other.daemon.open(&later, CAPACITY).root().unwrap(), b"base");

    let again = other.daemon.point(&copy, &other.acme.join("again"));
    assert_eq!(failure(again), (Code::InvalidArgument, Errno::EINVAL));
    assert_eq!(refused(&other.daemon.vms, &copy), Some(Errno::EROFS));
}

/// The environment of this test's process as another user ([`as_another_user`]): a socket of the
/// daemon's, mounted where a jail would have it, and a socket beside it anyone may connect to.
const JAILED_SOCKET: &str = "FXDAX_TEST_JAILED_SOCKET";
const OPEN_SOCKET: &str = "FXDAX_TEST_OPEN_SOCKET";

/// The user a jail's VM process runs as, as scripts/test-e2e.sh has it.
const JAIL_USER: u32 = 64000;

struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// TASK-155: the real daemon's own socket, whatever its umask, even none, is its user's alone: a
/// VM process of another user, with it bind-mounted into its mount namespace, is refused by the
/// kernel, where a socket beside it connects. Needs `sudo -n`, as Lima's VMs have it.
#[test]
fn the_default_socket_refuses_other_users() {
    refuses_other_users("the_default_socket_refuses_other_users", |sockets| sockets.vms);
}

/// TASK-159: so is its control socket, which can do everything.
#[test]
fn the_control_socket_refuses_other_users() {
    refuses_other_users("the_control_socket_refuses_other_users", |sockets| sockets.control);
}

/// The sockets of a daemon [`refuses_other_users`] runs.
struct Sockets {
    vms: PathBuf,
    control: PathBuf,
}

/// That the real daemon, under no umask, refuses a process of another user the socket of its that
/// `jailed` picks, mounted into the process's mount namespace: the test `test`, which this process
/// runs again as that user.
fn refuses_other_users(test: &str, jailed: fn(Sockets) -> PathBuf) {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt as _;
    if let Some(socket) = std::env::var_os(JAILED_SOCKET) {
        return as_another_user(socket.as_ref());
    }
    // Where the other user reaches, so that what refuses it is the socket.
    let dir = tempfile::Builder::new().prefix("fxdax-").tempdir_in("/tmp").unwrap();
    let everyone = |path: &Path, mode| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    everyone(dir.path(), 0o755);
    let (vms, control) = (dir.path().join("extentd.sock"), dir.path().join("extentd.control"));
    let mut daemon = std::process::Command::new(env!("CARGO_BIN_EXE_fxdax-extentd"));
    daemon.arg(&vms).env("FXDAX_EXTENTD_GRPC", &control);
    daemon.env("FXDAX_EXTENTD_REMOTE", "memory:///branches").env("FXDAX_EXTENTD_TOKEN", "1");
    // SAFETY: umask is async-signal-safe.
    unsafe {
        daemon.pre_exec(|| {
            libc::umask(0);
            Ok(())
        })
    };
    let _daemon = KillOnDrop(daemon.spawn().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while UnixStream::connect(&vms).is_err() || UnixStream::connect(&control).is_err() {
        assert!(std::time::Instant::now() < deadline, "the daemon never served");
        std::thread::yield_now();
    }
    let socket = jailed(Sockets { vms, control });

    let open = dir.path().join("open.sock");
    let _open = std::os::unix::net::UnixListener::bind(&open).unwrap();
    everyone(&open, 0o666);
    let jail = dir.path().join("jail");
    std::fs::create_dir(&jail).unwrap();
    everyone(&jail, 0o755);
    let jail_socket = jail.join("extentd.sock");
    std::fs::write(&jail_socket, b"").unwrap();
    let script = r#"mount --bind "$1" "$2" && exec env "$3" "$4" "$5" --exact "$6" --nocapture"#;
    let status = std::process::Command::new("sudo")
        .args(["-n", "unshare", "--mount", "--propagation", "private", "sh", "-c", script, "sh"])
        .arg(&socket)
        .arg(&jail_socket)
        .arg(format!("{JAILED_SOCKET}={}", jail_socket.display()))
        .arg(format!("{OPEN_SOCKET}={}", open.display()))
        .arg(std::env::current_exe().unwrap())
        .arg(test)
        .status()
        .unwrap();
    assert!(status.success(), "the other user connected, or failed to try: {status}");
    let metadata = std::fs::metadata(&socket).unwrap();
    // SAFETY: A plain system call.
    assert_eq!((metadata.mode() & 0o777, metadata.uid()), (0o600, unsafe { libc::getuid() }));
}

/// Root in a mount namespace of its own, it turns into the jail's user: what it runs needn't be
/// that user's to run. Then the kernel refuses it the daemon's socket, and not the one beside it.
fn as_another_user(socket: &Path) {
    use std::os::unix::net::UnixStream;
    // SAFETY: Plain system calls: this process does nothing else as root.
    unsafe {
        assert_eq!(libc::setgroups(0, std::ptr::null()), 0);
        assert_eq!(libc::setgid(JAIL_USER), 0);
        assert_eq!(libc::setuid(JAIL_USER), 0);
    }
    UnixStream::connect(std::env::var_os(OPEN_SOCKET).unwrap()).unwrap();
    let refused = UnixStream::connect(socket).unwrap_err();
    assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);
}
