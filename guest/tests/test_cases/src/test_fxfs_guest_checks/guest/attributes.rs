// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The checks of what a file has besides its data: extended attributes, ACLs, privileges, flags,
//! ID-mapped mounts and times.

use super::*;

/// fxdax: an extended attribute set reads as set while it is written back, not as before it
/// was, as a reader of overlayfs's opaque flag on a directory would find (TASK-79.51).
pub(super) fn test_xattrs_read_as_written_back() {
    use std::sync::atomic::{AtomicU64, Ordering};
    const ROUNDS: u64 = 500;
    let name = path("xattr-written-back");
    let file = fs::File::create(&name).unwrap();
    let (file_name, attribute) =
        (CString::new(name.as_str()).unwrap(), CString::new("user.written").unwrap());
    // Odd while it is set.
    let turn = AtomicU64::new(0);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while turn.load(Ordering::SeqCst) < 2 * ROUNDS {
                let before = turn.load(Ordering::SeqCst);
                let mut value = [0u8; 1];
                let len = unsafe {
                    libc::getxattr(
                        file_name.as_ptr(),
                        attribute.as_ptr(),
                        value.as_mut_ptr().cast(),
                        value.len(),
                    )
                };
                let error = std::io::Error::last_os_error();
                if before % 2 == 1 && turn.load(Ordering::SeqCst) == before {
                    assert_eq!(len, 1, "turn {before}: getxattr: {error}");
                }
            }
        });
        for _ in 0..ROUNDS {
            set_xattr(&name, "user.written", b"1");
            turn.fetch_add(1, Ordering::SeqCst);
            file.sync_all().unwrap();
            turn.fetch_add(1, Ordering::SeqCst);
            let removed = unsafe { libc::removexattr(file_name.as_ptr(), attribute.as_ptr()) };
            assert_eq!(removed, 0, "removexattr: {}", std::io::Error::last_os_error());
            file.sync_all().unwrap();
        }
    });
    drop(file);
    fs::remove_file(&name).unwrap();
}

/// fxdax: a full filesystem refuses an extended attribute it has no room for (ENOSPC), as ext4's
/// does, rather than keep it in memory and lose it behind fsync's 0 as its writeback fails: small
/// ones, which take only the journal's room, set until one is refused, which isn't there; each set
/// before it durable, read back remounted, as is one set once there is room again (TASK-199).
pub(super) fn test_xattr_on_full_filesystem_is_refused() {
    let name = path("xattr-full");
    fs::write(&name, b"x").unwrap();
    let filler = Removed(path("filler"));
    fill_until_enospc(&filler.0);
    let attribute = |n: usize| format!("user.x{n}");
    let refused = (0..100_000).find(|n| match try_set_xattr(&name, &attribute(*n), b"1") {
        Ok(()) => false,
        Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => true,
        Err(error) => panic!("setxattr {}: {error}", attribute(*n)),
    });
    let refused = refused.expect("a full filesystem took 100,000 extended attributes");
    assert_eq!(get_xattr_errno(&name, &attribute(refused)), Some(libc::ENODATA), "refused");
    drop(filler);
    set_xattr(&name, "user.after", b"2");
    fs::File::open(&name).unwrap().sync_all().unwrap();
    unmount();
    mount();
    for n in 0..refused {
        assert_eq!(get_xattr(&name, &attribute(n)), b"1", "{} of {refused}", attribute(n));
    }
    assert_eq!(get_xattr(&name, "user.after"), b"2");
    fs::remove_file(&name).unwrap();
}

/// fxdax: on a full filesystem, a creation in a directory with a default ACL fails whole (ENOSPC,
/// no name made) or makes its file or directory with the ACLs it inherits: they go in its
/// creation's transaction (TASK-79.29), as ext4 sets them in its creation's handle. Set after it,
/// in a transaction of their own as a setxattr's is (TASK-199), a creation whose ACLs then found no
/// room stood without them, the access its directory gives refused and its mode not theirs.
pub(super) fn test_inherited_acls_on_full_filesystem() {
    let dir = Removed(path("inherits-full"));
    fs::create_dir(&dir.0).unwrap();
    let default = posix_acl(&[
        (USER_OBJ, 6, UNDEFINED),
        (USER, 6, 1234),
        (GROUP_OBJ, 4, UNDEFINED),
        (MASK, 6, UNDEFINED),
        (OTHER, 4, UNDEFINED),
    ]);
    set_xattr(&dir.0, "system.posix_acl_default", &default);
    let filler = Removed(path("filler"));
    fill_until_enospc(&filler.0);
    // Files and directories in turn, each inheriting the default ACL as its access ACL (masked
    // by 0666 and 0777, which leave it as it is), a directory as its default ACL too.
    let refused = (0..100_000).find(|n| {
        let (name, made) = match n % 2 {
            0 => (format!("{}/f{n}", dir.0), fs::File::create(format!("{}/f{n}", dir.0)).map(drop)),
            _ => (format!("{}/d{n}", dir.0), fs::create_dir(format!("{}/d{n}", dir.0))),
        };
        match made {
            Ok(()) => {
                assert_eq!(get_xattr(&name, "system.posix_acl_access"), default, "{name}'s");
                if n % 2 == 1 {
                    assert_eq!(get_xattr(&name, "system.posix_acl_default"), default, "{name}'s");
                }
                false
            }
            Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => {
                let left = fs::symlink_metadata(&name).map(|meta| format!("{:o}", meta.mode()));
                assert!(left.is_err(), "a creation refused (ENOSPC) left {name}, mode {left:?}");
                true
            }
            Err(error) => panic!("creating {name}: {error}"),
        }
    });
    refused.expect("a full filesystem took 100,000 creations");
}

/// A file or directory, removed as this goes, and synced, its room free again: if a check that
/// filled the filesystem failed, for the checks after it.
struct Removed(String);

impl Drop for Removed {
    fn drop(&mut self) {
        // Not a panic, which in a panic's unwinding would abort before the first said why.
        let path = std::path::Path::new(&self.0);
        let removed = if path.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
        if let Err(error) = removed {
            eprintln!("removing {}: {error}", self.0);
        }
        unsafe { libc::sync() };
    }
}

/// The errno of getxattr(2) of `name` of `path`, which fails: `None` if it doesn't.
fn get_xattr_errno(path: &str, name: &str) -> Option<i32> {
    let (path, name) = (CString::new(path).unwrap(), CString::new(name).unwrap());
    let mut value = [0u8; 16];
    let len = unsafe {
        libc::getxattr(path.as_ptr(), name.as_ptr(), value.as_mut_ptr().cast(), value.len())
    };
    (len < 0).then(|| std::io::Error::last_os_error().raw_os_error().unwrap())
}

/// A file, `name`, written until the filesystem is full, a MiB at a time, then a page: what fitted
/// durable.
fn fill_until_enospc(name: &str) {
    let mut file = fs::File::create(name).unwrap();
    for block in [vec![0x5a; 1 << 20], vec![0x5a; 4096]] {
        loop {
            match file.write_all(&block) {
                Ok(()) => {}
                Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => break,
                Err(error) => panic!("filling {name}: {error}"),
            }
        }
    }
    file.sync_all().unwrap();
}

/// fxdax: extended attributes set keep none of the kernel's memory: setxattrs of names of their
/// own, 64,000 bytes each (Fxfs's most), until twice the guest's memory or the filesystem is full,
/// end with the OOM killer not run, each written as it is set, and refused (ENOSPC) once the
/// filesystem is full, which its 256 MiB are first. Kept in memory until written back, they took
/// the kernel's without bound, which no OOM kill frees (TASK-199).
pub(super) fn test_xattrs_bounded() {
    let memory = meminfo("MemTotal") * 1024;
    let killed = vmstat("oom_kill");
    let dir = Removed(path("xattrs-bounded"));
    fs::create_dir(&dir.0).unwrap();
    let value = vec![0xa5; 64000];
    let (mut set, mut refused) = (0u64, false);
    let enospc = |error: &std::io::Error| error.raw_os_error() == Some(libc::ENOSPC);
    // Fewer than the 448 an object holds too large to keep inline.
    'files: for n in 0.. {
        let file = format!("{}/f{n}", dir.0);
        match fs::write(&file, b"") {
            Ok(()) => {}
            Err(error) if enospc(&error) => {
                refused = true;
                break;
            }
            Err(error) => panic!("creating {file}, after {set} bytes: {error}"),
        }
        for attribute in 0..400 {
            match try_set_xattr(&file, &format!("user.big{attribute}"), &value) {
                Ok(()) => set += value.len() as u64,
                Err(error) if enospc(&error) => {
                    refused = true;
                    break 'files;
                }
                Err(error) => panic!("setxattr, after {set} bytes: {error}"),
            }
            if set >= 2 * memory {
                break 'files;
            }
        }
    }
    assert_eq!(vmstat("oom_kill"), killed, "OOM kills, after {set} bytes of extended attributes");
    assert!(
        refused,
        "{set} bytes of extended attributes set, of {memory} bytes' memory, none refused"
    );
}

/// `name`'s count in /proc/meminfo.
fn meminfo(name: &str) -> u64 {
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap();
    let line = meminfo.lines().find_map(|line| line.strip_prefix(&format!("{name}:")));
    line.unwrap().trim().trim_end_matches(" kB").parse().unwrap()
}

/// `name`'s count in /proc/vmstat.
fn vmstat(name: &str) -> u64 {
    let vmstat = fs::read_to_string("/proc/vmstat").unwrap();
    let line = vmstat.lines().find_map(|line| line.strip_prefix(&format!("{name} ")));
    line.unwrap().parse().unwrap()
}

/// fxdax: an inode whose writeback failed, as sync's or the flusher's, whose errors nothing
/// reports, stays dirty: the next fsync writes it back, or fails as it does, until it is durable
/// (fxfs.fail_write_inode). The VFS took it for clean, and the fsync after found nothing to write:
/// 0, and a chmod lost at the next eviction (TASK-199).
pub(super) fn test_failed_write_inode_reported() {
    let name = path("write-inode-failed");
    fs::write(&name, b"x").unwrap();
    fs::File::open(&name).unwrap().sync_all().unwrap();
    let ino = fs::metadata(&name).unwrap().ino();
    fs::set_permissions(&name, fs::Permissions::from_mode(0o600)).unwrap();
    {
        let _fail = Param::set("fail_write_inode", &ino.to_string());
        unsafe { libc::sync() };
        assert_injected("fail_write_inode");
    }
    fs::File::open(&name).unwrap().sync_all().unwrap();
    unmount();
    mount();
    assert_eq!(fs::metadata(&name).unwrap().permissions().mode() & 0o777, 0o600);
    // And an fsync's own writeback, failing, fails it.
    fs::set_permissions(&name, fs::Permissions::from_mode(0o640)).unwrap();
    {
        let _fail = Param::set("fail_write_inode", &ino.to_string());
        let failed = fs::File::open(&name).unwrap().sync_all().unwrap_err();
        assert_eq!(failed.raw_os_error(), Some(libc::EIO), "{failed}");
    }
    fs::File::open(&name).unwrap().sync_all().unwrap();
    unmount();
    mount();
    assert_eq!(fs::metadata(&name).unwrap().permissions().mode() & 0o777, 0o640);
    fs::remove_file(&name).unwrap();
}

// A POSIX ACL entry's tags, and the ID of those that name none.
const USER_OBJ: u16 = 0x01;
const USER: u16 = 0x02;
const GROUP_OBJ: u16 = 0x04;
const MASK: u16 = 0x10;
const OTHER: u16 = 0x20;
const UNDEFINED: u32 = u32::MAX;

/// POSIX ACLs: set and read back, the mode following them, and a directory's default ACL
/// inherited, in place of the umask, which applies without one.
pub(super) fn test_acls() {
    fs::write(path("acl-file"), b"acl").unwrap();
    let access = posix_acl(&[
        (USER_OBJ, 6, UNDEFINED),
        (USER, 6, 1234),
        (GROUP_OBJ, 4, UNDEFINED),
        (MASK, 6, UNDEFINED),
        (OTHER, 0, UNDEFINED),
    ]);
    set_xattr(&path("acl-file"), "system.posix_acl_access", &access);
    assert_eq!(get_xattr(&path("acl-file"), "system.posix_acl_access"), access);
    // The group's bits are the mask's.
    assert_eq!(fs::metadata(path("acl-file")).unwrap().mode() & 0o777, 0o660);
    // The ACL is enforced: user 1234, which it names, may read; user 1236 may not.
    let may_open = |uid: u32, flags: libc::c_int| {
        let file = CString::new(path("acl-file")).unwrap();
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork: {}", std::io::Error::last_os_error());
        if pid == 0 {
            // In the child: drop to `uid`, try, and say how it went.
            let status = unsafe {
                if libc::setgroups(0, std::ptr::null()) != 0
                    || libc::setresgid(uid, uid, uid) != 0
                    || libc::setresuid(uid, uid, uid) != 0
                {
                    2
                } else {
                    let fd = libc::open(file.as_ptr(), flags);
                    if fd >= 0 { 0 } else { 1 }
                }
            };
            unsafe { libc::_exit(status) };
        }
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert!(libc::WIFEXITED(status), "child {status:#x}");
        libc::WEXITSTATUS(status)
    };
    assert_eq!(may_open(1234, libc::O_RDONLY), 0, "the user the ACL names can't read");
    assert_eq!(may_open(1236, libc::O_RDONLY), 1, "another user could read");
    assert_eq!(may_open(1234, libc::O_WRONLY), 0, "the user the ACL names can't write");
    // fxdax: a chmod takes the group's write away, and the mask's with it, so the user the
    // ACL names may no longer write (TASK-79.96): chmod left the ACL as it was.
    fs::set_permissions(path("acl-file"), fs::Permissions::from_mode(0o640)).unwrap();
    assert_eq!(may_open(1234, libc::O_WRONLY), 1, "the user the ACL names can still write");
    assert_eq!(may_open(1234, libc::O_RDONLY), 0, "the user the ACL names can't read");
    let masked = posix_acl(&[
        (USER_OBJ, 6, UNDEFINED),
        (USER, 6, 1234),
        (GROUP_OBJ, 4, UNDEFINED),
        (MASK, 4, UNDEFINED),
        (OTHER, 0, UNDEFINED),
    ]);
    assert_eq!(get_xattr(&path("acl-file"), "system.posix_acl_access"), masked);

    let old_umask = unsafe { libc::umask(0o077) };
    fs::create_dir(path("acl-dir")).unwrap();
    let default =
        posix_acl(&[(USER_OBJ, 7, UNDEFINED), (GROUP_OBJ, 5, UNDEFINED), (OTHER, 5, UNDEFINED)]);
    set_xattr(&path("acl-dir"), "system.posix_acl_default", &default);
    fs::write(path("acl-dir/inherits"), b"x").unwrap();
    fs::write(path("umasked"), b"x").unwrap();
    unsafe { libc::umask(old_umask) };
    // The default ACL, not the umask, limits the new file's mode.
    assert_eq!(fs::metadata(path("acl-dir/inherits")).unwrap().mode() & 0o777, 0o644);
    // Without one, the umask does.
    assert_eq!(fs::metadata(path("umasked")).unwrap().mode() & 0o777, 0o600);
}

/// SB_NOSEC: a file's first write skips looking for what a write removes, until it has some;
/// then a write removes it: security.capability, and the set-user-ID bit, for a writer without
/// CAP_FSETID.
pub(super) fn test_write_removes_privileges() {
    let file = path("privileged");
    fs::write(&file, b"before").unwrap();
    // A v2 capability: CAP_NET_RAW, effective.
    let mut capability = vec![0u8; 20];
    capability[..4].copy_from_slice(&0x0200_0001u32.to_le_bytes());
    capability[4..8].copy_from_slice(&(1u32 << 13).to_le_bytes());
    let (c_file, name) = (CString::new(file.as_str()).unwrap(), c"security.capability");
    let set = unsafe {
        libc::setxattr(c_file.as_ptr(), name.as_ptr(), capability.as_ptr().cast(), 20, 0)
    };
    assert_eq!(set, 0, "setxattr: {}", std::io::Error::last_os_error());
    fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(b", after").unwrap();
    let got = unsafe { libc::getxattr(c_file.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0) };
    let error = std::io::Error::last_os_error().raw_os_error();
    assert_eq!((got, error), (-1, Some(libc::ENODATA)), "security.capability kept");

    // Set-user-ID, writable by anyone: written by another user, it is not any more.
    fs::set_permissions(&file, fs::Permissions::from_mode(0o4777)).unwrap();
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork: {}", std::io::Error::last_os_error());
    if pid == 0 {
        let dropped = unsafe { libc::setgid(1000) == 0 && libc::setuid(1000) == 0 };
        let wrote = dropped
            && fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .and_then(|mut file| file.write_all(b", by another"))
                .is_ok();
        unsafe { libc::_exit(if wrote { 0 } else { 1 }) };
    }
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0, "{status:#x}");
    assert_eq!(fs::metadata(&file).unwrap().mode() & 0o7777, 0o777, "set-user-ID kept");
    assert_eq!(fs::read(&file).unwrap(), b"before, after, by another");
}

const FS_IOC_GETFLAGS: libc::c_ulong = 0x80086601;
const FS_IOC_SETFLAGS: libc::c_ulong = 0x40086602;
const FS_IMMUTABLE_FL: libc::c_int = 0x10;
const FS_APPEND_FL: libc::c_int = 0x20;
const FS_NODUMP_FL: libc::c_int = 0x40;
const FS_NOATIME_FL: libc::c_int = 0x80;

/// `path`'s attribute flags (lsattr).
fn get_flags(path: &str) -> libc::c_int {
    let file = fs::File::open(path).unwrap();
    let mut flags: libc::c_int = 0;
    let ret = unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_GETFLAGS as _, &mut flags) };
    assert_eq!(ret, 0, "FS_IOC_GETFLAGS {path}: {}", std::io::Error::last_os_error());
    flags
}

/// Sets `path`'s attribute flags (chattr).
fn set_flags(path: &str, flags: libc::c_int) {
    let file = fs::File::open(path).unwrap();
    let ret = unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_SETFLAGS as _, &flags) };
    assert_eq!(ret, 0, "FS_IOC_SETFLAGS {path}: {}", std::io::Error::last_os_error());
}

/// TASK-26.19.4: chattr's flags, immutable, append-only, no-dump and no-atime: lsattr reads
/// them back, the kernel holds files to them, and Fxfs keeps them: as they are once the inode
/// is evicted and read in again, and once the filesystem is mounted again.
pub(super) fn test_file_attributes() {
    let (immutable, append) = (path("immutable"), path("append-only"));
    fs::write(&immutable, b"kept").unwrap();
    fs::write(&append, b"first").unwrap();
    set_flags(&immutable, FS_IMMUTABLE_FL | FS_NODUMP_FL);
    set_flags(&append, FS_APPEND_FL | FS_NOATIME_FL);
    let refused = |result: std::io::Result<()>| result.err().and_then(|err| err.raw_os_error());
    let write = |path: &str| fs::OpenOptions::new().write(true).open(path).map(drop);
    assert_eq!(refused(write(&immutable)), Some(libc::EPERM), "write to immutable");
    assert_eq!(refused(fs::remove_file(&immutable)), Some(libc::EPERM), "unlink immutable");
    let renamed = fs::rename(&immutable, path("immutable-renamed"));
    assert_eq!(refused(renamed), Some(libc::EPERM), "rename immutable");
    assert_eq!(refused(write(&append)), Some(libc::EPERM), "write to append-only");
    let mut appending = fs::OpenOptions::new().append(true).open(&append).unwrap();
    appending.write_all(b", then appended").unwrap();
    assert_eq!(refused(appending.set_len(0)), Some(libc::EPERM), "truncate append-only");
    drop(appending);
    let flags = || (get_flags(&immutable), get_flags(&append));
    let set = (FS_IMMUTABLE_FL | FS_NODUMP_FL, FS_APPEND_FL | FS_NOATIME_FL);
    assert_eq!(flags(), set);
    evict(&immutable);
    evict(&append);
    assert_eq!(flags(), set, "read in again");
    unmount();
    mount();
    assert_eq!(flags(), set, "mounted again");
    assert_eq!(fs::read(&immutable).unwrap(), b"kept");
    assert_eq!(fs::read(&append).unwrap(), b"first, then appended");
    // Cleared, they are files as any other.
    set_flags(&immutable, 0);
    set_flags(&append, 0);
    fs::write(&immutable, b"changed").unwrap();
    fs::remove_file(&immutable).unwrap();
    fs::remove_file(&append).unwrap();
}

/// TASK-26.19.8: an idmapped mount of the filesystem (FS_ALLOW_IDMAP), mapping IDs 0 to 999 to
/// 10000 to 10999: a file root owns is 10000's through it; one a caller of 10005 makes through
/// it is 5's in the filesystem; and root, whom it maps to nothing, makes none (EOVERFLOW).
pub(super) fn test_idmapped_mount() {
    const OPEN_TREE_CLONE: libc::c_uint = 1;
    const MOUNT_ATTR_IDMAP: u64 = 0x0010_0000;
    const MOVE_MOUNT_F_EMPTY_PATH: libc::c_uint = 4;
    /// `struct mount_attr`.
    #[repr(C)]
    struct MountAttr {
        attr_set: u64,
        attr_clr: u64,
        propagation: u64,
        userns_fd: u64,
    }
    let check = |ret: libc::c_long, what: &str| {
        assert!(ret >= 0, "{what}: {}", std::io::Error::last_os_error());
        ret
    };
    let dir = path("idmapped");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
    fs::write(format!("{dir}/root's"), b"root's").unwrap();

    // A user namespace mapping 0 to 999 to 10000 to 10999: a child's, which waits in it.
    let (mut ready, mut go) = ([0; 2], [0; 2]);
    assert_eq!(unsafe { libc::pipe(ready.as_mut_ptr()) }, 0);
    assert_eq!(unsafe { libc::pipe(go.as_mut_ptr()) }, 0);
    let child = unsafe { libc::fork() };
    if child == 0 {
        unsafe {
            let mut byte = 0u8;
            if libc::unshare(libc::CLONE_NEWUSER) != 0
                || libc::write(ready[1], (&raw const byte).cast(), 1) != 1
                || libc::read(go[0], (&raw mut byte).cast(), 1) != 1
            {
                libc::_exit(1);
            }
            libc::_exit(0);
        }
    }
    let mut byte = 0u8;
    assert_eq!(unsafe { libc::read(ready[0], (&raw mut byte).cast(), 1) }, 1);
    for map in ["uid_map", "gid_map"] {
        fs::write(format!("/proc/{child}/{map}"), b"0 10000 1000\n").unwrap();
    }
    let userns = fs::File::open(format!("/proc/{child}/ns/user")).unwrap();
    let mnt = CString::new(MNT).unwrap();
    let tree = check(
        unsafe {
            libc::syscall(
                libc::SYS_open_tree,
                libc::AT_FDCWD,
                mnt.as_ptr(),
                OPEN_TREE_CLONE | libc::O_CLOEXEC as libc::c_uint,
            )
        },
        "open_tree",
    ) as libc::c_int;
    let attr = MountAttr {
        attr_set: MOUNT_ATTR_IDMAP,
        attr_clr: 0,
        propagation: 0,
        userns_fd: userns.as_raw_fd() as u64,
    };
    let empty = c"";
    check(
        unsafe {
            libc::syscall(
                libc::SYS_mount_setattr,
                tree,
                empty.as_ptr(),
                libc::AT_EMPTY_PATH,
                &raw const attr,
                std::mem::size_of::<MountAttr>(),
            )
        },
        "mount_setattr",
    );
    let target = "/mnt/idmapped";
    fs::create_dir_all(target).unwrap();
    let target_c = CString::new(target).unwrap();
    check(
        unsafe {
            libc::syscall(
                libc::SYS_move_mount,
                tree,
                empty.as_ptr(),
                libc::AT_FDCWD,
                target_c.as_ptr(),
                MOVE_MOUNT_F_EMPTY_PATH,
            )
        },
        "move_mount",
    );
    unsafe {
        libc::close(tree);
        assert_eq!(libc::write(go[1], (&raw const byte).cast(), 1), 1);
        let mut status = 0;
        assert_eq!(libc::waitpid(child, &mut status, 0), child);
        assert_eq!(status, 0, "the user namespace's child");
    }

    let through = format!("{target}/idmapped");
    assert_eq!(fs::metadata(format!("{through}/root's")).unwrap().uid(), 10000);
    let made = format!("{through}/5's");
    let maker = unsafe { libc::fork() };
    if maker == 0 {
        unsafe {
            libc::setfsgid(10005);
            libc::setfsuid(10005);
        }
        let code = match fs::write(&made, b"5's") {
            Ok(()) => 0,
            Err(err) => err.raw_os_error().unwrap_or(255),
        };
        unsafe { libc::_exit(code) };
    }
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(maker, &mut status, 0) }, maker);
    assert_eq!(libc::WEXITSTATUS(status), 0, "10005's create through the idmapped mount");
    let owner = |path: &str| {
        let metadata = fs::metadata(path).unwrap();
        (metadata.uid(), metadata.gid())
    };
    assert_eq!(owner(&format!("{dir}/5's")), (5, 5), "in the filesystem");
    assert_eq!(owner(&made), (10005, 10005), "through the idmapped mount");
    let unmapped = fs::write(format!("{through}/root-made"), b"");
    assert_eq!(unmapped.err().and_then(|err| err.raw_os_error()), Some(libc::EOVERFLOW));

    let ret = unsafe { libc::umount2(target_c.as_ptr(), libc::MNT_DETACH) };
    assert_eq!(ret, 0, "umount {target}: {}", std::io::Error::last_os_error());
    fs::remove_dir_all(&dir).unwrap();
}

/// TASK-26.19.8: multigrain timestamps (FS_MGTIME): a change after stat read the ctime before
/// has a later ctime, however quickly the changes follow one another, where a clock tick's
/// granularity would leave many the same.
pub(super) fn test_multigrain_times() {
    let file = path("multigrain");
    fs::write(&file, b"").unwrap();
    let ctime = || {
        let metadata = fs::metadata(&file).unwrap();
        (metadata.ctime(), metadata.ctime_nsec())
    };
    let mut before = ctime();
    for change in 0..200 {
        let mode = if change % 2 == 0 { 0o600 } else { 0o644 };
        fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        let after = ctime();
        assert!(after > before, "change {change}: ctime {after:?}, before it {before:?}");
        before = after;
    }
    fs::remove_file(&file).unwrap();
}

/// fxdax: a time past what Fxfs holds, 64 bits of nanoseconds, is clamped as it is set, as the
/// superblock says: not only once the inode is read in again (TASK-79.55).
pub(super) fn test_timestamps_clamped() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    const SECONDS: i64 = 13_000_000_000;
    const KEPT: i64 = 9_223_372_035;
    let name = path("timestamps-clamped");
    fs::write(&name, b"a time").unwrap();
    let c_name = std::ffi::CString::new(std::ffi::OsStr::new(&name).as_bytes()).unwrap();
    for (asked, kept) in [(SECONDS, KEPT), (-SECONDS, -KEPT)] {
        let time = libc::timespec { tv_sec: asked, tv_nsec: 0 };
        let set =
            unsafe { libc::utimensat(libc::AT_FDCWD, c_name.as_ptr(), [time, time].as_ptr(), 0) };
        assert_eq!(set, 0, "utimensat: {}", std::io::Error::last_os_error());
        assert_eq!(fs::metadata(&name).unwrap().mtime(), kept, "as it was set");
        evict(&name);
        assert_eq!(fs::metadata(&name).unwrap().mtime(), kept, "read in again");
    }
    fs::remove_file(&name).unwrap();
}
