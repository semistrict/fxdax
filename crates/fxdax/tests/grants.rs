// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-43.4: tenants granted VM sockets of their own through the control API. A connection's
//! tenant is the socket it came in on: its VM process names stores in the tenant's directory
//! alone. A revoked tenant's socket takes no more connections. A tenant's socket works
//! bind-mounted into a jail, as the host's VM manager mounts it.

#![cfg(target_os = "linux")]

mod common;

use common::daemon::{CAPACITY, Daemon, failure, write};
use common::{cache, copies, pattern};
use fxdax::{Errno, RemoteExtentStore};
use std::path::Path;
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
