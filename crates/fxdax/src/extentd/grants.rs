// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The tenants granted VM sockets (TASK-43.4), which the control API's Grant and Revoke make and
//! take away: a socket of the tenant's own, in the daemon's directory of them, for the host's VM
//! manager to bind-mount into the tenant's jails. A connection's tenant is the socket it came in
//! on. Grants live as long as the daemon: after a restart, the VM manager grants again.

use super::server::{Tenant, serve_tenant};
use crate::Errno;
use std::collections::HashMap;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The longest name a tenant has.
const NAME_MAX: usize = 64;

/// The tenants granted, by name, and the directory their sockets are in.
pub(super) struct Grants {
    sockets: PathBuf,
    granted: Mutex<HashMap<String, Granted>>,
}

/// A tenant granted its socket.
struct Granted {
    tenant: Arc<Tenant>,
    socket: PathBuf,
    /// The socket's listener, which a revoke shuts down, so that its thread stops accepting.
    listener: UnixListener,
}

impl Grants {
    /// No tenants yet, whose sockets go in the directory `sockets`.
    pub fn new(sockets: PathBuf) -> Self {
        Self { sockets, granted: Mutex::new(HashMap::new()) }
    }

    /// Grants the tenant `name`, whose stores are in `dir`, its socket, given to `uid` and `gid`
    /// (the daemon's own, where not given), and returns its path: a new tenant's, which a thread
    /// of its own serves; or, granted already, its socket, given again (`EEXIST` with another
    /// directory). A tenant revoked whose VM processes live on is the one granted again, its
    /// socket new: those it connects then share its arena with them, and the stores they have open
    /// (TASK-79.36), which, another tenant's, they could not.
    pub fn grant(
        &self,
        name: &str,
        dir: &Path,
        uid: Option<u32>,
        gid: Option<u32>,
    ) -> Result<PathBuf, Errno> {
        check_name(name)?;
        let dir = dir.canonicalize().map_err(errno)?;
        let mut granted = self.granted.lock().unwrap();
        if let Some(grant) = granted.get(name) {
            if grant.tenant.dir() != dir {
                return Err(Errno::EEXIST);
            }
            give(&grant.socket, uid, gid)?;
            return Ok(grant.socket.clone());
        }
        let tenant = match super::memory::alive(name, &dir) {
            Some(tenant) => tenant,
            None => Tenant::new(name, &dir).map_err(errno)?,
        };
        let socket = self.sockets.join(format!("{name}.sock"));
        remove_stale(&socket)?;
        let listener = UnixListener::bind(&socket).map_err(errno)?;
        let served = give(&socket, uid, gid)
            .and_then(|()| listener.try_clone().map_err(errno))
            .and_then(|accepting| {
                let serving = tenant.clone();
                std::thread::Builder::new()
                    .name("extentd tenant".into())
                    .spawn(move || serve_tenant(accepting, serving))
                    .map_err(errno)
            });
        if let Err(errno) = served {
            let _ = std::fs::remove_file(&socket);
            return Err(errno);
        }
        let grant = Granted { tenant, socket: socket.clone(), listener };
        granted.insert(name.to_owned(), grant);
        Ok(socket)
    }

    /// Revokes the tenant `name`'s grant (`ENOENT` if it has none): its socket goes, and takes no
    /// more connections. The tenant goes once its VM processes connected do.
    pub fn revoke(&self, name: &str) -> Result<(), Errno> {
        let grant = self.granted.lock().unwrap().remove(name).ok_or(Errno::ENOENT)?;
        if let Err(err) = std::fs::remove_file(&grant.socket) {
            log::warn!("extentd: removing {}: {err}", grant.socket.display());
        }
        // Its thread's accept fails with `EINVAL`, and it stops.
        use std::os::fd::AsRawFd;
        // SAFETY: Plain system call on the listener's descriptor, which `grant` keeps open.
        if unsafe { libc::shutdown(grant.listener.as_raw_fd(), libc::SHUT_RDWR) } != 0 {
            log::warn!(
                "extentd: shutting tenant {name}'s socket down: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }
}

/// `EINVAL` unless `name` is a tenant's: letters, digits, `-`, `_` and `.`, not first, at most
/// [`NAME_MAX`], so that its socket's name is plain.
pub(super) fn check_name(name: &str) -> Result<(), Errno> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    let fits = (1..=NAME_MAX).contains(&name.len()) && !name.starts_with('.');
    if fits && name.chars().all(allowed) { Ok(()) } else { Err(Errno::EINVAL) }
}

/// Gives the socket at `socket` to `uid` and `gid`, where given, connectable by them alone.
fn give(socket: &Path, uid: Option<u32>, gid: Option<u32>) -> Result<(), Errno> {
    std::os::unix::fs::chown(socket, uid, gid).map_err(errno)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660)).map_err(errno)
}

/// Removes the socket at `socket` that a daemon before this one left, whose grants went with it:
/// `EEXIST` if something else is there.
fn remove_stale(socket: &Path) -> Result<(), Errno> {
    match std::fs::symlink_metadata(socket) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            std::fs::remove_file(socket).map_err(errno)
        }
        Ok(_) => Err(Errno::EEXIST),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(errno(err)),
    }
}

/// The errno of `err`, or `EIO`.
fn errno(err: std::io::Error) -> Errno {
    err.raw_os_error().map_or(Errno::EIO, Errno::from_raw)
}

#[cfg(test)]
mod tests {
    use super::check_name;
    use crate::Errno;

    /// Tenants' names name their sockets, plainly.
    #[test]
    fn names_are_plain() {
        for name in ["acme", "acme-2", "a_b.c", &"a".repeat(64)] {
            assert_eq!(check_name(name), Ok(()), "{name}");
        }
        for name in ["", ".", "..", ".hidden", "a/b", "a b", "ä", &"a".repeat(65)] {
            assert_eq!(check_name(name), Err(Errno::EINVAL), "{name}");
        }
    }
}
