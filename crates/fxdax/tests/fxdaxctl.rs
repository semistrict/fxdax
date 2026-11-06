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
    env: Vec<(&'static str, String)>,
}

impl Fxdaxctl {
    /// fxdaxctl acting on stores itself, as a host whose bucket is in its process's memory, of
    /// placement 1.
    fn local() -> Self {
        Self::with_env(&HOST)
    }

    /// fxdaxctl acting on stores itself, with `env`, and nothing else of its kind, in its
    /// environment.
    fn with_env(env: &[(&'static str, &str)]) -> Self {
        let env = env.iter().map(|&(name, value)| (name, value.to_owned())).collect();
        Self { daemon: None, env }
    }

    /// fxdaxctl acting through a daemon of its own, serving in `dir`, a host as tests have it
    /// ([`HOST`]), with `env` in its environment too.
    #[cfg(target_os = "linux")]
    fn through_daemon(dir: &Path, env: &[(&str, &str)]) -> Self {
        Self { daemon: Some(daemon(dir, env, std::process::Stdio::inherit())), env: Vec::new() }
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
        command.envs(self.env.iter().map(|(name, value)| (name, value)));
        let output = command.output().expect("running fxdaxctl failed");
        match output.status.success() {
            true => Ok(String::from_utf8(output.stdout).unwrap()),
            false => Err(String::from_utf8_lossy(&output.stderr).into_owned()),
        }
    }
}

/// An extent daemon serving in `dir`, a host as tests have it ([`HOST`]), with `env` in its
/// environment too, and its standard error `stderr`, once it serves its control API: the control
/// socket, and the daemon, which goes with this.
#[cfg(target_os = "linux")]
fn daemon(dir: &Path, env: &[(&str, &str)], stderr: std::process::Stdio) -> (PathBuf, KillOnDrop) {
    use std::time::{Duration, Instant};
    let control = dir.join("control.sock");
    let mut command = Command::new(env!("CARGO_BIN_EXE_fxdax-extentd"));
    command.arg(dir.join("vms.sock")).env("FXDAX_EXTENTD_GRPC", &control).stderr(stderr);
    let child = command.envs(HOST).envs(env.iter().copied()).spawn().unwrap();
    let child = KillOnDrop(child);
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::os::unix::net::UnixStream::connect(&control).is_err() {
        assert!(Instant::now() < deadline, "the daemon never served");
        std::thread::sleep(Duration::from_millis(20));
    }
    (control, child)
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

/// TASK-39.4: `fxdaxctl delete` deletes a store whose fork lives: its branch in its bucket and its
/// directory, the fork living on, a store of its own, reading what it inherited in the bucket
/// (TASK-165.7). Before, the store's directory stayed, holding what the fork shared of it.
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
    assert!(!path.exists(), "the store's directory stayed");
    let forked = ExtentStore::open(&fork).unwrap();
    assert_eq!(forked.branch(), branch);
    assert_eq!(read(&forked, extent), [7; BLOCK]);
    let own = ExtentId { branch, sequence: 1 };
    forked.write(own, 0, &[VolatileSlice::from(&[9; BLOCK][..])]).unwrap();
    forked.commit(None).unwrap();
    drop(forked);
    // TASK-49: what the fork wrote, and what it inherited, which it shares.
    let stat = fxdaxctl.run(&[Path::new("stat"), &fork]).unwrap();
    assert!(stat.contains(&format!("used {}\n", 2 * BLOCK)), "{stat}");
    assert!(stat.contains(&format!("own {BLOCK}\ninherited {BLOCK}\n")), "{stat}");
    // Deleted again: nothing is there.
    assert!(fxdaxctl.run(&[Path::new("delete"), &path]).is_err(), "deleted nothing again");
    let forked = ExtentStore::open(&fork).unwrap();
    assert_eq!(read(&forked, extent), [7; BLOCK]);
    drop(forked);

    fxdaxctl.run(&[Path::new("delete"), &fork]).unwrap();
    assert!(!fork.exists());
}

/// TASK-154: `fxdaxctl drop` deletes what `take` made, a copy of a branch this host holds nothing
/// of, and a second finds nothing there.
fn copies_are_dropped(fxdaxctl: Fxdaxctl, dir: &Path) {
    let path = dir.join("taken");
    let (take, capacity, branch) = (Path::new("take"), Path::new("16777216"), Path::new("ab"));
    fxdaxctl.run(&[take, &path, capacity, branch]).unwrap();
    assert!(path.exists(), "nothing was taken");
    fxdaxctl.run(&[Path::new("drop"), &path]).unwrap();
    assert!(!path.exists(), "the copy stayed");
    assert!(fxdaxctl.run(&[Path::new("drop"), &path]).is_err(), "a second drop found a store");
}

#[test]
fn copies_are_dropped_locally() {
    let dir = tempfile::tempdir().unwrap();
    copies_are_dropped(Fxdaxctl::with_env(&[]), dir.path());
}

#[cfg(target_os = "linux")]
#[test]
fn copies_are_dropped_through_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    copies_are_dropped(Fxdaxctl::through_daemon(dir.path(), &[]), dir.path());
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

/// fxdaxctl imports an image where no daemon is, into a store keeping its branch in the host's
/// bucket, and forks it there too, as fxfs-guest-import forks the base it imports for its guest:
/// the fork is the bucket's (TASK-165.7), made by the uploader the store, opened there, keeps its
/// branch with. The fork holds the image's file, read from the bucket; the base is as it was. The
/// host's bucket is a file bucket: what the import uploaded has left the store, which a bucket in
/// the import's memory would have taken with it (TASK-165.8).
#[test]
fn images_imported_where_no_daemon_is_fork_there() {
    use futures::executor::block_on;
    use fxdax::import::testing::{Entry, Kind, layer};
    use fxdax::{Filesystem, Storage};
    let dir = tempfile::tempdir().unwrap();
    let bucket = format!("file://{}/", dir.path().canonicalize().unwrap().join("bucket").display());
    let fxdaxctl =
        Fxdaxctl::with_env(&[("FXDAX_EXTENTD_REMOTE", &bucket), ("FXDAX_EXTENTD_TOKEN", "1")]);
    let image = dir.path().join("layer.tar");
    std::fs::write(&image, layer(&[Entry::new("hello", Kind::File(b"hello\n"))]).unwrap()).unwrap();
    let (base, fork) = (dir.path().join("base"), dir.path().join("fork"));
    let run = |args: &[&dyn AsRef<Path>]| {
        let args: Vec<&Path> = args.iter().map(|arg| arg.as_ref()).collect();
        fxdaxctl.run(&args)
    };
    run(&[&"import", &base, &"67108864", &image]).unwrap();
    let printed = run(&[&"fork", &base, &fork]).unwrap();
    let branch = u64::from_str_radix(printed.trim(), 16).unwrap();
    assert_eq!(ExtentStore::open(&fork).unwrap().branch(), branch);
    // The file `hello` of the store at `path`, as its filesystem has it.
    let hello = |path: &Path| {
        block_on(async {
            let fs = Filesystem::mount(Storage::open(path).unwrap()).await.unwrap();
            let (file, _) = fs.root().lookup(b"hello").await.unwrap();
            let mut read = vec![0; 16];
            let len = file.read(0, &mut read).await.unwrap();
            read.truncate(len);
            drop(file);
            fs.close().await.unwrap();
            read
        })
    };
    assert_eq!(hello(&fork), b"hello\n");
    assert_eq!(hello(&base), b"hello\n");
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

/// fxdax TASK-111.4: `stats --json` prints one snapshot through the daemon: its build and
/// counters, and its VM process's, as its guest answered, with the guest's counters, histograms and
/// build; without `--json`, the same as lines of a path and a value. Without the daemon, refused.
#[cfg(target_os = "linux")]
#[test]
fn stats_are_one_snapshot_of_the_daemon_and_its_vms() {
    let dir = tempfile::tempdir().unwrap();
    let fxdaxctl = Fxdaxctl::through_daemon(dir.path(), &[]);
    let path = dir.path().join("store");
    let store = fxdax::RemoteExtentStore::connect(dir.path().join("vms.sock"), &path, 1 << 24);
    let store = std::sync::Arc::new(store.unwrap());
    // Its guest answers each ask, off the thread the ask comes on.
    let answering = std::sync::Arc::downgrade(&store);
    store.on_stats_asked(move |ask| {
        let store = answering.clone();
        std::thread::spawn(move || {
            let guest = "seals 3\nseals_hist 0,2,1\ncommit abc\n";
            if let Some(store) = store.upgrade() {
                store.answer_stats(ask, Some(guest)).unwrap();
            }
        });
        true
    });
    let run = |args: &[&str]| {
        let args: Vec<&Path> = args.iter().map(Path::new).collect();
        fxdaxctl.run(&args).unwrap()
    };

    let json: serde_json::Value = serde_json::from_str(&run(&["stats", "--json"])).unwrap();
    assert_eq!(json["extentd"]["build"], fxdax::BUILD_COMMIT);
    assert_eq!(json["extentd"]["gauges"]["connections"], 1);
    assert!(json["taken_unix_ns"].as_u64() > Some(0), "{json}");
    let vm = &json["vms"][0];
    assert_eq!(vm["path"], path.canonicalize().unwrap().to_str().unwrap());
    assert_eq!(vm["branch"], format!("{:x}", store.branch()));
    assert_eq!((&vm["follower"], &vm["answered"]), (&false.into(), &true.into()));
    assert_eq!(vm["vmm"]["build"], fxdax::BUILD_COMMIT);
    assert_eq!(vm["vmm"]["counters"]["stage_ops"], 0);
    assert_eq!(vm["window"], serde_json::Value::Null);
    let guest = &vm["guest"];
    assert_eq!(guest["build"], "abc");
    assert_eq!(guest["counters"], serde_json::json!({ "seals": 3 }));
    assert_eq!(guest["histograms"], serde_json::json!({ "seals": [0, 2, 1] }));

    let lines = run(&["stats"]);
    let line = |at: &str| lines.lines().find_map(|line| line.strip_prefix(&format!("{at} ")));
    assert_eq!(line("vms.0.guest.counters.seals"), Some("3"), "{lines}");
    assert_eq!(line("vms.0.guest.histograms.seals.1"), Some("2"), "{lines}");
    assert_eq!(line("extentd.build"), Some(fxdax::BUILD_COMMIT), "{lines}");

    drop(store);
    let refused = Fxdaxctl::local().run(&[Path::new("stats"), Path::new("--json")]);
    assert!(refused.unwrap_err().contains("FXDAX_EXTENTD_GRPC"));
}

/// What `child`, whose standard error is piped, writes there once sent SIGQUIT (TASK-44.13): its
/// threads' stacks, from their heading to the first line after it naming `waiting`, a function a
/// thread of it waits in. Its standard error is read to its end, so that its writes never fail.
#[cfg(target_os = "linux")]
fn stacks_on_sigquit(child: &mut std::process::Child, waiting: &str) -> String {
    use std::io::BufRead;
    use std::time::{Duration, Instant};
    const HEADING: &str = "fxdax: the threads' stacks, on SIGQUIT:";
    let stderr = child.stderr.take().expect("its standard error, piped");
    let (sent, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
            // Once the test has what it needs, nothing receives the rest.
            let _ = sent.send(line);
        }
    });
    // SAFETY: Signals a process this test started.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGQUIT) }, 0);
    let deadline = Instant::now() + Duration::from_secs(60);
    let (mut printed, mut heading) = (String::new(), None);
    loop {
        let line = match lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(line) => line,
            Err(err) => panic!("no stack naming {waiting} ({err}); it wrote:\n{printed}"),
        };
        if heading.is_none() && line.starts_with(HEADING) {
            heading = Some(printed.len());
        }
        printed += &line;
        printed.push('\n');
        if let Some(heading) = heading.filter(|_| line.contains(waiting)) {
            return printed.split_off(heading);
        }
    }
}

/// TASK-44.13: SIGQUIT has the daemon write its threads' stacks, naming where they wait: here, the
/// thread that waits for SIGTERM. It goes on running, and SIGTERM still stops it, successfully.
#[cfg(target_os = "linux")]
#[test]
fn the_daemon_writes_its_stacks_on_sigquit() {
    let dir = tempfile::tempdir().unwrap();
    let (_, mut child) = daemon(dir.path(), &[], std::process::Stdio::piped());
    let stacks = stacks_on_sigquit(&mut child.0, "drain_on_sigterm");
    assert_eq!(child.0.try_wait().unwrap(), None, "SIGQUIT stopped the daemon:\n{stacks}");
    // SAFETY: Signals the daemon this test started.
    assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGTERM) }, 0);
    let status = child.0.wait().unwrap();
    assert!(status.success(), "{status}");
}

/// TASK-44.13: SIGQUIT has fxdaxctl write its threads' stacks, naming where they wait, as an import
/// that hung needed (TASK-98): here, its call through a daemon that never answers. It goes on
/// waiting.
#[cfg(target_os = "linux")]
#[test]
fn fxdaxctl_writes_its_stacks_on_sigquit() {
    use std::io::Read;
    use std::time::{Duration, Instant};
    let dir = tempfile::tempdir().unwrap();
    // A control socket that takes connections and never answers them.
    let control = dir.path().join("control.sock");
    let listener = std::os::unix::net::UnixListener::bind(&control).unwrap();
    listener.set_nonblocking(true).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_fxdaxctl"))
        .args([Path::new("stat"), &dir.path().join("store")])
        .env("FXDAX_EXTENTD_GRPC", &control)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = KillOnDrop(child);
    // Connected, so past the first thing it does, handling SIGQUIT, which would end it before.
    let deadline = Instant::now() + Duration::from_secs(30);
    let _connection = loop {
        match listener.accept() {
            Ok((connection, _)) => break connection,
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(status) = child.0.try_wait().unwrap() {
                    let mut said = String::new();
                    child.0.stderr.take().unwrap().read_to_string(&mut said).unwrap();
                    panic!("fxdaxctl stopped before it connected, {status}: {said}");
                }
                assert!(Instant::now() < deadline, "fxdaxctl never connected");
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(err) => panic!("accepting fxdaxctl's connection: {err}"),
        }
    };
    let stacks = stacks_on_sigquit(&mut child.0, "through");
    assert_eq!(child.0.try_wait().unwrap(), None, "SIGQUIT stopped fxdaxctl:\n{stacks}");
}

/// TASK-48: SIGTERM stops the daemon: it exits, successfully.
#[cfg(target_os = "linux")]
#[test]
fn the_daemon_stops_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let mut fxdaxctl = Fxdaxctl::through_daemon(dir.path(), &[]);
    let (_, child) = fxdaxctl.daemon.as_mut().unwrap();
    // SAFETY: Signals the daemon this test started.
    assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGTERM) }, 0);
    let status = child.0.wait().unwrap();
    assert!(status.success(), "{status}");
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

/// Commands with more arguments than they take are refused, with the usage.
#[test]
fn commands_with_more_arguments_print_the_usage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let fxdaxctl = Fxdaxctl::with_env(&[]);
    let image = dir.path().join("layer.tar");
    for args in [
        ["take", "67108864", "1f", "2", "3"].as_slice(),
        ["import", "67108864", "layer.tar", "ref", "more"].as_slice(),
    ] {
        let mut all: Vec<&Path> = vec![Path::new(args[0]), &path];
        all.extend(args[1..].iter().map(|arg| match *arg {
            "layer.tar" => image.as_path(),
            arg => Path::new(arg),
        }));
        let error = fxdaxctl.run(&all).expect_err("ran with more arguments");
        assert!(error.contains("usage: fxdaxctl take DIR CAPACITY BRANCH [TOKEN]"), "{error}");
        assert!(!path.exists());
    }
}

/// TASK-111.7: `fxdaxctl --version` prints the commit it was built of, the library's.
#[test]
fn the_version_is_the_build_commit() {
    let printed = Fxdaxctl::with_env(&[]).run(&[Path::new("--version")]).unwrap();
    assert_eq!(printed, format!("fxdaxctl {}\n", fxdax::BUILD_COMMIT));
}

/// Without a daemon, a store grows, or keeps its capacity, here; it shrinks only through the
/// daemon serving its VM, whose guest must agree.
#[test]
fn stores_grow_here_and_shrink_only_through_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    drop(ExtentStore::create(&path, 1 << 24).unwrap());
    let fxdaxctl = Fxdaxctl::local();
    let resize = |capacity: u64| {
        let capacity = capacity.to_string();
        fxdaxctl.run(&[Path::new("resize"), &path, Path::new(&capacity)])
    };
    let capacity = || ExtentStore::open(&path).unwrap().info().unwrap().capacity;
    resize(1 << 24).unwrap();
    assert_eq!(capacity(), 1 << 24);
    resize(1 << 25).unwrap();
    assert_eq!(capacity(), 1 << 25);
    let error = resize(1 << 24).expect_err("shrank a store here");
    assert!(error.contains("shrinking a store needs its guest to agree"), "{error}");
    assert_eq!(capacity(), 1 << 25);
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
