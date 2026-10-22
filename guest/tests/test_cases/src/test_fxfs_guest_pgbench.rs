// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The sandbox team's load (fxdax's TASK-57, TASK-62): PostgreSQL's pgbench makes its database, at
//! scale 50, on a fork of a base store, through a DAX window, its branches in a bucket 25 ms away,
//! as theirs was. It broke Fxfs twice: a kernel panic in a truncate (TASK-58), then the primary
//! key's parallel build reading back other data than its workers wrote (TASK-59). A base VM makes
//! the base's filesystem; the host forks it through a daemon of its own; a VM as theirs boots on
//! the fork, makes the database, and checks it: every account there once. Then pgbench runs on it,
//! as theirs did, where Fxfs's journal shut down on a write to a sealed extent (TASK-91.1). With
//! a DAX window (fxfs-guest-pgbench-dax) or without (fxfs-guest-pgbench).

use macros::{guest, host};

pub struct TestFxfsGuestPgbench {
    pub dax: bool,
}

/// pgbench's scale: 100,000 accounts a unit.
#[cfg_attr(feature = "host", allow(dead_code))]
const SCALE: u64 = 50;

/// The pgbench runs after the load, and each one's seconds, as the sandbox team's.
#[cfg_attr(feature = "host", allow(dead_code))]
const RUNS: usize = 3;
#[cfg_attr(feature = "host", allow(dead_code))]
const RUN_SECS: u64 = 60;

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use std::ffi::{CString, OsStr};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    /// The stores' capacity, and the fork's VM's DAX window if it has one, as the sandbox team's.
    const CAPACITY: u64 = 16 << 30;
    const WINDOW: u64 = 2 << 30;

    const CONTAINERFILE: &str = "\
FROM alpine:3.22
RUN apk add --no-cache postgresql postgresql-contrib
";

    /// A VM of the guest's `role`, of `memory` MiB, on the store at `store`, which the daemon at
    /// `extentd` serves, run in Firecracker.
    fn run(
        test_setup: &TestSetup,
        root: &Path,
        role: &str,
        store: &Path,
        extentd: &Path,
        memory: u32,
        window: u64,
    ) -> anyhow::Result<Firecracker> {
        let mut vm = Vm::new(2, memory)?;
        vm.guest_kernel()?;
        vm.extent_store_served("extents", store, CAPACITY, window, extentd);
        vm.env(&CString::new(format!("FXDAX_PGBENCH_ROLE={role}"))?);
        vm.spawn_api(root, &test_setup.test_case, role, &Vm::api_socket(role))
    }

    /// Samples, every 2 s until `stop`, what the daemon at `socket` did and has left to upload of
    /// the store at `store`: its stats file, `stats`, and the store's upload lag, appended to
    /// `samples` with the seconds since it started: to see where a run's time goes (TASK-91.2).
    fn sample(
        socket: PathBuf,
        store: PathBuf,
        stats: PathBuf,
        samples: PathBuf,
        stop: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<anyhow::Result<()>> {
        std::thread::spawn(move || {
            use std::io::Write;
            let control = crate::common::control(&socket)?;
            let mut out = std::fs::File::create(samples)?;
            let started = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(2));
                let stat = control.stat_store(&store)?;
                writeln!(
                    out,
                    "t={} pending_upload_bytes={} oldest_pending_ms={}",
                    started.elapsed().as_secs(),
                    stat.pending_upload_bytes,
                    stat.oldest_pending_ms
                )?;
                out.write_all(&std::fs::read(&stats).unwrap_or_default())?;
            }
            Ok(())
        })
    }

    /// Waits for `firecracker`'s VM to end, and passes on what its guest said.
    fn wait(firecracker: Firecracker) -> anyhow::Result<()> {
        let (status, console) = firecracker.wait()?;
        crate::vm::split_console(std::fs::File::open(&console)?)?;
        anyhow::ensure!(status.success(), "a VM exited {status}");
        Ok(())
    }

    impl TestFxfsGuestPgbench {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let root = setup_rootfs(test_setup)?;
            // A daemon of its own, whose bucket answers 25 ms late, as the sandbox team's RustFS
            // did, or as FXDAX_REMOTE_LATENCY_MS says; its socket's path short, as Unix sockets'
            // must be.
            let sockets = tempdir::TempDir::new("fxdax-pgbench")?;
            let socket = sockets.path().join("extentd.sock");
            let latency = std::env::var_os("FXDAX_REMOTE_LATENCY_MS");
            let stats = dir.join("extentd-stats.txt");
            let env = [
                ("FXDAX_REMOTE_LATENCY_MS", latency.as_deref().unwrap_or(OsStr::new("25"))),
                ("FXDAX_EXTENTD_STATS", stats.as_os_str()),
            ];
            let _daemon = crate::common::spawn_daemon(test_setup, &socket, "extentd", &env)?;
            let (base, fork) = (dir.join("base"), dir.join("fork"));
            wait(run(test_setup, &root, "base", &base, &socket, 512, 0)?)?;
            crate::common::control(&socket)?.fork_store(&base, &fork)?;
            let window = if self.dax { WINDOW } else { 0 };
            let stop = Arc::new(AtomicBool::new(false));
            let samples = dir.join("samples.txt");
            let sampler = sample(socket.clone(), fork.clone(), stats, samples, stop.clone());
            let loaded = wait(run(test_setup, &root, "load", &fork, &socket, 4096, window)?);
            stop.store(true, Ordering::Relaxed);
            sampler.join().expect("the sampler panicked")?;
            loaded
        }
    }

    impl Test for TestFxfsGuestPgbench {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn rootfs_image(&self) -> Option<&'static str> {
            Some(CONTAINERFILE)
        }

        fn timeout_secs(&self) -> u64 {
            1200
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            if let Err(err) = self.run(&test_setup) {
                eprintln!("{err:#}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8(stdout).unwrap();
            let expected = format!("OK base\nOK load {}\nOK runs {RUNS}\n", SCALE * 100_000);
            if output == expected {
                TestOutcome::Pass
            } else {
                TestOutcome::Fail(format!("expected {expected:?}, got {output:?}"))
            }
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;

    use nix::libc;
    use std::ffi::CString;
    use std::fs;
    use std::process::{Command, Output, Stdio};

    const MNT: &str = "/mnt/fxfs";

    fn mount() {
        fs::create_dir_all(MNT).unwrap();
        let (source, target, fstype) = (
            CString::new("extents").unwrap(),
            CString::new(MNT).unwrap(),
            CString::new("fxfs").unwrap(),
        );
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
    }

    fn unmount() {
        let target = CString::new(MNT).unwrap();
        assert_eq!(unsafe { libc::umount(target.as_ptr()) }, 0, "umount");
    }

    /// Runs `command` as the user postgres, which PostgreSQL runs as, and returns its output,
    /// which must say it succeeded; what it says on stderr, and when it started and how long it
    /// took, go to the console, to follow a slow run.
    fn as_postgres(command: &str) -> Output {
        let start = std::time::Instant::now();
        // The guest's uptime, to set beside the host's samples of the daemon.
        let uptime = fs::read_to_string("/proc/uptime").unwrap_or_default();
        let uptime = uptime.split(' ').next().unwrap_or_default();
        eprintln!("pgbench: at {uptime} s: {command}");
        let output = Command::new("su")
            .args(["postgres", "-s", "/bin/sh", "-c", command])
            .stderr(Stdio::inherit())
            .output()
            .expect("su");
        eprintln!("pgbench: {:.1?}, {}", start.elapsed(), output.status);
        assert!(
            output.status.success(),
            "{command}: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout)
        );
        output
    }

    impl Test for TestFxfsGuestPgbench {
        fn in_guest(self: Box<Self>) {
            mount();
            match std::env::var("FXDAX_PGBENCH_ROLE").unwrap().as_str() {
                // The base's filesystem, made as it mounts, with a directory for the database.
                "base" => {
                    fs::create_dir_all(format!("{MNT}/pgdata")).unwrap();
                    unmount();
                    println!("OK base");
                }
                "load" => {
                    let data = format!("{MNT}/pgdata");
                    let socket = "/run/postgresql";
                    fs::create_dir_all(socket).unwrap();
                    Command::new("chown")
                        .args(["postgres:postgres", &data, socket])
                        .status()
                        .unwrap();
                    as_postgres(&format!("initdb -D {data} -A trust"));
                    let options =
                        format!("-c shared_buffers=256MB -k {socket} -c listen_addresses=''");
                    // The server's output to a log of its own: it would hold `as_postgres`'s pipe
                    // open, which waits for its end.
                    as_postgres(&format!(
                        "pg_ctl -D {data} -l {data}/server.log -o \"{options}\" -w start"
                    ));
                    as_postgres(&format!("createdb -h {socket} bench"));
                    as_postgres(&format!("pgbench -h {socket} -i -q -s {SCALE} bench"));
                    // Every account once: the primary key's build found one twice (TASK-59).
                    let count = "select count(*), count(distinct aid) from pgbench_accounts";
                    let counted = as_postgres(&format!("psql -h {socket} -At -c '{count}' bench"));
                    let accounts = SCALE * 100_000;
                    assert_eq!(
                        String::from_utf8_lossy(&counted.stdout).trim(),
                        format!("{accounts}|{accounts}")
                    );
                    println!("OK load {accounts}");
                    // Then as the sandbox team's runs, each of which must end well: pgbench says
                    // so (its tps, on the console, to compare).
                    for _ in 0..RUNS {
                        let run = as_postgres(&format!(
                            "pgbench -h {socket} -c 4 -j 2 -T {RUN_SECS} bench"
                        ));
                        eprint!("{}", String::from_utf8_lossy(&run.stdout));
                    }
                    println!("OK runs {RUNS}");
                    as_postgres(&format!("pg_ctl -D {data} -w -m fast stop"));
                    unmount();
                }
                role => panic!("no role {role}"),
            }
        }
    }
}
