// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of fxdaxctl, fxdax's tool for VMs' disks: each acting on stores itself, as where no
//! extent daemon runs, and, on Linux, through an extent daemon's control API (TASK-43.5).

use fxdax::{ExtentId, ExtentStore, VolatileSlice, VolatileSliceMut};
use std::path::{Path, PathBuf};
use std::process::Command;

const BLOCK: usize = ExtentStore::BLOCK_SIZE as usize;

/// The environment of a host of the multi-node system, as tests have it: a bucket in each
/// process's memory, and placement 1.
const HOST: [(&str, &str); 2] =
    [("FXDAX_EXTENTD_REMOTE", "memory:///branches"), ("FXDAX_EXTENTD_TOKEN", "1")];

/// fxdaxctl, acting on stores itself, with `env` in its environment, or through a daemon,
/// `fxdax-extentd`, of its own.
struct Fxdaxctl {
    /// The daemon's control socket, and the daemon, which goes with this.
    daemon: Option<(PathBuf, KillOnDrop)>,
    env: Vec<(&'static str, &'static str)>,
}

impl Fxdaxctl {
    /// fxdaxctl acting on stores itself, as a host whose bucket is in its process's memory, of
    /// placement 1.
    fn local() -> Self {
        Self::with_env(&HOST)
    }

    /// fxdaxctl acting on stores itself, with `env`, and nothing else of its kind, in its
    /// environment.
    fn with_env(env: &[(&'static str, &'static str)]) -> Self {
        Self { daemon: None, env: env.to_vec() }
    }

    /// fxdaxctl acting through a daemon of its own, serving in `dir`, a host as tests have it
    /// ([`HOST`]), with `env` in its environment too.
    #[cfg(target_os = "linux")]
    fn through_daemon(dir: &Path, env: &[(&str, &str)]) -> Self {
        use std::time::{Duration, Instant};
        let control = dir.join("control.sock");
        let mut command = Command::new(env!("CARGO_BIN_EXE_fxdax-extentd"));
        command.arg(dir.join("vms.sock")).env("FXDAX_EXTENTD_GRPC", &control);
        let child = command.envs(HOST).envs(env.iter().copied()).spawn().unwrap();
        let child = KillOnDrop(child);
        let deadline = Instant::now() + Duration::from_secs(30);
        while std::os::unix::net::UnixStream::connect(&control).is_err() {
            assert!(Instant::now() < deadline, "the daemon never served");
            std::thread::sleep(Duration::from_millis(20));
        }
        Self { daemon: Some((control, child)), env: Vec::new() }
    }

    /// Runs fxdaxctl with `args`: what it prints, or, if it fails, what it says.
    fn run(&self, args: &[&Path]) -> Result<String, String> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fxdaxctl"));
        command.args(args);
        for name in ["FXDAX_EXTENTD_GRPC", "FXDAX_EXTENTD_REMOTE", "FXDAX_EXTENTD_TOKEN"] {
            command.env_remove(name);
        }
        if let Some((control, _)) = &self.daemon {
            command.env("FXDAX_EXTENTD_GRPC", control);
        }
        command.envs(self.env.iter().copied());
        let output = command.output().expect("running fxdaxctl failed");
        match output.status.success() {
            true => Ok(String::from_utf8(output.stdout).unwrap()),
            false => Err(String::from_utf8_lossy(&output.stderr).into_owned()),
        }
    }
}

struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn read(store: &ExtentStore, extent: ExtentId) -> Vec<u8> {
    let mut data = vec![0; BLOCK];
    store.read(extent, 0, &mut [VolatileSliceMut::from(&mut data[..])]).unwrap();
    data
}

/// TASK-39.4: `fxdaxctl delete` deletes a store whose fork lives, which keeps what it shares of
/// it, and deletes the store's directory with the fork.
fn stores_are_deleted_while_their_forks_live(fxdaxctl: Fxdaxctl, dir: &Path) {
    let (path, fork) = (dir.join("store"), dir.join("fork"));
    let store = ExtentStore::create(&path, 1 << 24).unwrap();
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    store.write(extent, 0, &[VolatileSlice::from(&[7; BLOCK][..])]).unwrap();
    store.commit(None).unwrap();
    drop(store);
    let printed = fxdaxctl.run(&[Path::new("fork"), &path, &fork]).unwrap();
    let branch = u64::from_str_radix(printed.trim(), 16).unwrap();

    fxdaxctl.run(&[Path::new("delete"), &path]).unwrap();
    assert!(path.exists(), "the store's directory went while its fork lives");
    let error = ExtentStore::open(&path).err().expect("opened a deleted store");
    assert_eq!(
        format!("{error:#}"),
        "the store was deleted: its directory holds its forks' data alone"
    );
    let forked = ExtentStore::open(&fork).unwrap();
    assert_eq!(forked.branch(), branch);
    assert_eq!(read(&forked, extent), [7; BLOCK]);
    let own = ExtentId { branch, sequence: 1 };
    forked.write(own, 0, &[VolatileSlice::from(&[9; BLOCK][..])]).unwrap();
    forked.commit(None).unwrap();
    drop(forked);
    // Deleted again, as after a deletion that stopped partway: nothing more goes.
    fxdaxctl.run(&[Path::new("delete"), &path]).unwrap();
    assert!(path.exists(), "the store's directory went while its fork lives");
    let forked = ExtentStore::open(&fork).unwrap();
    assert_eq!(read(&forked, extent), [7; BLOCK]);
    drop(forked);

    fxdaxctl.run(&[Path::new("delete"), &fork]).unwrap();
    assert!(!fork.exists() && !path.exists());
}

#[test]
fn stores_are_deleted_while_their_forks_live_locally() {
    let dir = tempfile::tempdir().unwrap();
    stores_are_deleted_while_their_forks_live(Fxdaxctl::local(), dir.path());
}

#[cfg(target_os = "linux")]
#[test]
fn stores_are_deleted_while_their_forks_live_through_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    stores_are_deleted_while_their_forks_live(
        Fxdaxctl::through_daemon(dir.path(), &[]),
        dir.path(),
    );
}

/// fxdaxctl changes no store whose branches another process has open, as an extent daemon serving
/// them does: it waits a while for them to close, then refuses, saying `refusal`.
fn stores_open_elsewhere_are_left_alone(fxdaxctl: Fxdaxctl, dir: &Path, refusal: &str) {
    let path = dir.join("store");
    let store = ExtentStore::create(&path, 1 << 24).unwrap();
    let error = fxdaxctl.run(&[Path::new("delete"), &path]).expect_err("deleted an open store");
    assert!(error.contains(refusal), "{error}");
    assert!(ExtentStore::open(dir.join("store")).is_err(), "opened twice");
    drop(store);
    fxdaxctl.run(&[Path::new("delete"), &path]).unwrap();
    assert!(!path.exists());
}

#[test]
fn stores_open_elsewhere_are_left_alone_locally() {
    let dir = tempfile::tempdir().unwrap();
    let refusal = "another process has the store's branches open";
    stores_open_elsewhere_are_left_alone(Fxdaxctl::local(), dir.path(), refusal);
}

#[cfg(target_os = "linux")]
#[test]
fn stores_open_elsewhere_are_left_alone_through_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let fxdaxctl = Fxdaxctl::through_daemon(dir.path(), &[]);
    stores_open_elsewhere_are_left_alone(fxdaxctl, dir.path(), "EBUSY (errno 16)");
}

/// Every command through the daemon: a store imported, made a public base, grown, forked, synced
/// to object storage, its fork's branch taken to a new store placed after it, and deleted.
#[cfg(target_os = "linux")]
#[test]
fn commands_through_the_daemon() {
    use fxdax::import::testing::{Entry, Kind, layer};
    let dir = tempfile::tempdir().unwrap();
    let env = [("FXDAX_EXTENTD_REMOTE", "memory:///branches"), ("FXDAX_EXTENTD_TOKEN", "1")];
    let fxdaxctl = Fxdaxctl::through_daemon(dir.path(), &env);
    let image = dir.path().join("layer.tar");
    std::fs::write(&image, layer(&[Entry::new("hello", Kind::File(b"hello\n"))]).unwrap()).unwrap();
    let (base, fork, taken) =
        (dir.path().join("base"), dir.path().join("fork"), dir.path().join("taken"));
    let run = |args: &[&dyn AsRef<Path>]| {
        let args: Vec<&Path> = args.iter().map(|arg| arg.as_ref()).collect();
        fxdaxctl.run(&args).unwrap()
    };

    run(&[&"import", &base, &"67108864", &image]);
    run(&[&"public", &base]);
    run(&[&"resize", &base, &"134217728"]);
    let branch = run(&[&"fork", &base, &fork]).trim().to_owned();
    run(&[&"sync", &fork]);
    run(&[&"take", &taken, &"67108864", &branch.as_str(), &"2"]);
    run(&[&"place", &taken, &"3"]);
    run(&[&"delete", &taken]);
    assert!(!taken.exists());

    // As the daemon left them, which serves none of them now.
    drop(fxdaxctl);
    let base = ExtentStore::open(&base).unwrap();
    assert_eq!(base.info().unwrap().capacity, 134217728);
    assert!(base.is_public());
    assert_eq!(format!("{:x}", ExtentStore::open(&fork).unwrap().branch()), branch);
}

/// TASK-45: without a daemon, fxdaxctl changes no existing store's branch unless it has the bucket
/// and this host's placement token; it makes new stores anywhere.
#[test]
fn existing_stores_change_only_with_a_bucket_and_a_token() {
    use fxdax::import::testing::{Entry, Kind, layer};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    drop(ExtentStore::create(&path, 1 << 24).unwrap());
    let delete = [Path::new("delete"), &path];

    let error = Fxdaxctl::with_env(&[]).run(&delete).expect_err("deleted with no bucket");
    assert!(error.contains("FXDAX_EXTENTD_REMOTE names no bucket"), "{error}");
    let bucket = [("FXDAX_EXTENTD_REMOTE", "memory:///branches")];
    let error = Fxdaxctl::with_env(&bucket).run(&delete).expect_err("deleted with no token");
    assert!(error.contains("FXDAX_EXTENTD_TOKEN names no placement token"), "{error}");
    let zero = [("FXDAX_EXTENTD_REMOTE", "memory:///branches"), ("FXDAX_EXTENTD_TOKEN", "0")];
    let error = Fxdaxctl::with_env(&zero).run(&delete).expect_err("deleted with token 0");
    assert!(error.contains("FXDAX_EXTENTD_TOKEN names no placement token"), "{error}");
    assert!(path.exists());

    // New stores, made where no bucket is.
    let image = dir.path().join("layer.tar");
    std::fs::write(&image, layer(&[Entry::new("hello", Kind::File(b"hello\n"))]).unwrap()).unwrap();
    let imported = dir.path().join("imported");
    let import = [Path::new("import"), &imported, Path::new("67108864"), &image];
    Fxdaxctl::with_env(&[]).run(&import).unwrap();
    let taken = dir.path().join("taken");
    Fxdaxctl::with_env(&[])
        .run(&[Path::new("take"), &taken, Path::new("67108864"), Path::new("1f")])
        .unwrap();
    assert!(ExtentStore::open(&imported).is_ok() && taken.exists());

    // With both.
    Fxdaxctl::local().run(&delete).unwrap();
    assert!(!path.exists());
}

/// TASK-45: the daemon refuses to start without the bucket and its placement token, saying why.
#[cfg(target_os = "linux")]
#[test]
fn the_daemon_needs_a_bucket_and_a_token() {
    let dir = tempfile::tempdir().unwrap();
    let start = |env: &[(&str, &str)]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fxdax-extentd"));
        command.arg(dir.path().join("vms.sock"));
        for name in ["FXDAX_EXTENTD_REMOTE", "FXDAX_EXTENTD_TOKEN"] {
            command.env_remove(name);
        }
        let output = command.envs(env.iter().copied()).output().unwrap();
        assert!(!output.status.success(), "the daemon started");
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    let error = start(&[]);
    assert!(error.contains("FXDAX_EXTENTD_REMOTE names no bucket"), "{error}");
    let error = start(&[("FXDAX_EXTENTD_REMOTE", "memory:///branches")]);
    assert!(error.contains("FXDAX_EXTENTD_TOKEN names no placement token"), "{error}");
    let error = start(&[("FXDAX_EXTENTD_REMOTE", "memory:///b"), ("FXDAX_EXTENTD_TOKEN", "0")]);
    assert!(error.contains("FXDAX_EXTENTD_TOKEN names no placement token"), "{error}");
    assert!(!dir.path().join("vms.sock").exists(), "it listened");
}
