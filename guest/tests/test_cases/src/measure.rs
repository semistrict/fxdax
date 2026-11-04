// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what a VM costs its host's memory, measured when its guest asks (TASK-164.5). The guest
//! connects to the host over vsock, on [`PORT`], and says `measure PHASE NAME=VALUE...`, its own
//! memory then; the host says, on its standard error, the test's log, the host's memory then, as
//! `memory: vm=VM phase=PHASE NAME=VALUE...`, which scripts/pgbench-gate.py reads, and answers
//! `ok` once the test lets the guest go on. VM 0 is none: the host before the first VM boots.
//! Every value is in bytes, but for counts' (`pages`, `faults`, `total`):
//!
//! - `host_used`: the host's memory not available (/proc/meminfo's MemTotal less MemAvailable):
//!   what the VMs, the daemon and all else hold, each page once, its page cache left out;
//! - `vmm_rss`, `vmm_pss`, `vmm_anon` and `vmm_shmem`: the VM process's resident memory, its
//!   proportional share of it, and of that its anonymous memory (the guest's RAM it touched) and
//!   its shared memory (the daemon's pages its DAX window maps) (/proc/PID/smaps_rollup);
//! - `extentd_pss`: the daemon process's share of its own;
//! - `extentd_*`: some of the daemon's metrics (fxdax::extentd::metrics): the bytes its arenas
//!   hold, every tenant's extents' pages (`extentd_memory_bytes`), the pages the windows map, its
//!   faults, and any gauge of private pages it keeps (copy-on-write DAX's, TASK-164.1);
//! - `guest_*`: what the guest said of itself: its memory (/proc/meminfo), faults (/proc/vmstat),
//!   and its Fxfs's counts of copy-on-write DAX, its private pages among them (`guest_fxfs_*`).
//!
//! And whether the guest's DAX writes copy on write (`fxfs.dax_cow`, doc-13), which a case may ask
//! for, the guest says on its console ([`DAX_COW`]).

use macros::{guest, host};
use std::collections::BTreeMap;

/// The vsock port a guest asks the host to measure it on.
pub const PORT: u32 = 5056;

/// The phase a guest that asks to be measured more than once says last.
pub const LAST: &str = "end";

/// In a guest's environment, that its case asks its DAX writes to copy on write, which it fails
/// without.
const DAX_COW: &str = "FXDAX_DAX_COW";

/// /proc/meminfo's `text`, each value by its name, in bytes, but for counts, which have no unit.
pub fn meminfo(text: &str) -> BTreeMap<String, u64> {
    (text.lines())
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            let value = value.trim();
            let bytes = match value.strip_suffix(" kB") {
                Some(kib) => kib.trim().parse::<u64>().ok()? << 10,
                None => value.parse().ok()?,
            };
            Some((name.to_owned(), bytes))
        })
        .collect()
}

#[host]
mod host {
    use super::*;
    use crate::vm::{Firecracker, Vm};
    use anyhow::Context;
    use std::fmt::Write as _;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    /// The daemon's metrics reported, by their names less `fxdax_extentd_`: besides them, any gauge
    /// of private pages.
    const REPORTED: [&str; 6] = [
        "memory_bytes",
        "window_pages",
        "window_mapped_bytes",
        "faults_total",
        "fault_stores_total",
        "page_ins_total",
    ];

    /// An address on the loopback for an extent daemon's metrics (FXDAX_EXTENTD_METRICS), free as
    /// this returns.
    pub fn metrics_address() -> anyhow::Result<SocketAddr> {
        Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?)
    }

    /// Has the guest of `vm` copy on write as its DAX writes (`fxfs.dax_cow=1`, doc-13), and fail
    /// unless its kernel does.
    pub fn ask_dax_cow(vm: &mut Vm) -> anyhow::Result<()> {
        vm.module_param(c"fxfs.dax_cow=1");
        vm.env(&std::ffi::CString::new(format!("{DAX_COW}=1"))?);
        Ok(())
    }

    /// The host's memory, and the extent daemon's, of process `daemon`, which serves its metrics at
    /// `metrics`.
    pub struct HostMemory {
        daemon: u32,
        metrics: SocketAddr,
    }

    impl HostMemory {
        pub fn new(daemon: u32, metrics: SocketAddr) -> Self {
            Self { daemon, metrics }
        }

        /// Says the host's memory now, on standard error: of VM `vm` (0 before the first), at
        /// `phase`, its VM process `vmm` if it runs, and what its guest said of itself, `guest`.
        pub fn say(
            &self,
            vm: usize,
            phase: &str,
            vmm: Option<u32>,
            guest: &str,
        ) -> anyhow::Result<()> {
            let mut line = format!("memory: vm={vm} phase={phase} host_used={}", host_used()?);
            if let Some(vmm) = vmm {
                let vmm = rollup(vmm).context("the VM process's memory")?;
                for (name, field) in [
                    ("vmm_rss", "Rss"),
                    ("vmm_pss", "Pss"),
                    ("vmm_anon", "Pss_Anon"),
                    ("vmm_shmem", "Pss_Shmem"),
                ] {
                    let bytes =
                        vmm.get(field).with_context(|| format!("no {field} in smaps_rollup"))?;
                    write!(line, " {name}={bytes}")?;
                }
            }
            let daemon = rollup(self.daemon).context("the daemon's memory")?;
            write!(line, " extentd_pss={}", daemon.get("Pss").context("no Pss in smaps_rollup")?)?;
            for (name, value) in gauges(self.metrics)? {
                write!(line, " {name}={value}")?;
            }
            if !guest.is_empty() {
                write!(line, " {guest}")?;
            }
            eprintln!("{line}");
            Ok(())
        }
    }

    /// The host's memory not available: MemTotal less MemAvailable.
    fn host_used() -> anyhow::Result<u64> {
        let memory = meminfo(&std::fs::read_to_string("/proc/meminfo")?);
        let (total, available) = (memory.get("MemTotal"), memory.get("MemAvailable"));
        let (Some(total), Some(available)) = (total, available) else {
            anyhow::bail!("/proc/meminfo has no MemTotal or MemAvailable");
        };
        Ok(total.saturating_sub(*available))
    }

    /// What /proc/PID/smaps_rollup says of process `pid`'s memory, in bytes, by name.
    fn rollup(pid: u32) -> anyhow::Result<BTreeMap<String, u64>> {
        let path = format!("/proc/{pid}/smaps_rollup");
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
        // Its lines but the first, the mappings' range, which has no size, are as meminfo's.
        Ok(meminfo(&text))
    }

    /// The daemon's metrics at `address` that are [`REPORTED`], or are of private pages, each as
    /// `extentd_NAME`: its unlabelled ones.
    fn gauges(address: SocketAddr) -> anyhow::Result<Vec<(String, u64)>> {
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))
            .with_context(|| format!("connecting to the daemon's metrics at {address}"))?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n")?;
        // It closes the connection once it has answered.
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        let (head, body) = response.split_once("\r\n\r\n").context("the metrics' response")?;
        anyhow::ensure!(head.starts_with("HTTP/1.1 200"), "the daemon's metrics: {head}");
        Ok((body.lines())
            .filter_map(|line| {
                let (name, value) = line.split_once(' ')?;
                let name = name.strip_prefix("fxdax_extentd_")?;
                let reported = REPORTED.contains(&name) || name.contains("private");
                if name.contains('{') || !reported {
                    return None;
                }
                Some((format!("extentd_{name}"), value.trim().parse().ok()?))
            })
            .collect())
    }

    /// The vsock of a VM, to give it ([`Vm::vsock`]), on which its guest asks to be measured: the
    /// host's end of [`PORT`].
    pub struct Requests {
        uds: PathBuf,
        path: PathBuf,
        listener: UnixListener,
    }

    impl Requests {
        /// The vsock of the VM `name` of this process's: in the temporary directory, as a socket's
        /// path is short.
        pub fn new(name: &str) -> anyhow::Result<Self> {
            let uds =
                std::env::temp_dir().join(format!("fxdax-{}-{name}.vsock", std::process::id()));
            let path = PathBuf::from(format!("{}_{PORT}", uds.display()));
            let listener = UnixListener::bind(&path)
                .with_context(|| format!("listening at {}", path.display()))?;
            Ok(Self { uds, path, listener })
        }

        /// The path of the vsock's Unix socket, Firecracker's `uds_path`.
        pub fn uds(&self) -> &Path {
            &self.uds
        }

        /// The next request of the guest of `firecracker`'s VM: an error if the VM exits first.
        pub fn next(&self, firecracker: &mut Firecracker) -> anyhow::Result<Request> {
            let stream = firecracker.accept(&self.listener)?;
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line)?;
            let asked = (line.trim_end().strip_prefix("measure "))
                .with_context(|| format!("the guest asked {line:?}"))?;
            let (phase, guest) = asked.split_once(' ').unwrap_or((asked, ""));
            Ok(Request { phase: phase.to_owned(), guest: guest.to_owned(), stream })
        }
    }

    impl Drop for Requests {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(&self.uds);
        }
    }

    /// A guest's request to be measured, which waits for its answer.
    pub struct Request {
        /// What the guest is at.
        pub phase: String,
        /// What it said of itself, as `NAME=VALUE` separated by spaces.
        pub guest: String,
        stream: UnixStream,
    }

    impl Request {
        /// Lets the guest go on.
        pub fn answer(self) -> anyhow::Result<()> {
            (&self.stream).write_all(b"ok\n").context("answering the guest")
        }
    }
}

#[host]
pub use host::*;

#[guest]
mod guest {
    use super::*;
    use std::fs;
    use std::io::{BufRead, BufReader, Write};

    /// Asks the host to measure its memory and this guest's, at `phase`, and waits until it lets
    /// it go on.
    pub fn measure(phase: &str) {
        use nix::sys::socket::{AddressFamily, SockFlag, SockType, VsockAddr, connect, socket};
        use std::os::fd::AsRawFd as _;
        let socket = socket(AddressFamily::Vsock, SockType::Stream, SockFlag::empty(), None)
            .expect("a vsock socket");
        connect(socket.as_raw_fd(), &VsockAddr::new(2, PORT)).expect("connecting to the host");
        let mut stream = fs::File::from(socket);
        writeln!(stream, "measure {phase} {}", itself()).unwrap();
        let mut answer = String::new();
        BufReader::new(&stream).read_line(&mut answer).unwrap();
        assert_eq!(answer, "ok\n", "the host's answer at {phase}");
    }

    /// This guest's memory, and the page faults it took so far, as `guest_NAME=VALUE` separated by
    /// spaces.
    fn itself() -> String {
        let memory = meminfo(&fs::read_to_string("/proc/meminfo").unwrap());
        let vmstat = fs::read_to_string("/proc/vmstat").unwrap();
        let count = |name: &str| {
            (vmstat.lines())
                .find_map(|line| line.strip_prefix(name)?.strip_prefix(' ')?.parse::<u64>().ok())
                .unwrap_or_else(|| panic!("/proc/vmstat has no {name}"))
        };
        let mut said: Vec<String> = [
            ("total", "MemTotal"),
            ("available", "MemAvailable"),
            ("cached", "Cached"),
            ("anon", "AnonPages"),
            ("shmem", "Shmem"),
        ]
        .iter()
        .map(|(name, field)| {
            let bytes =
                memory.get(*field).unwrap_or_else(|| panic!("/proc/meminfo has no {field}"));
            format!("guest_{name}={bytes}")
        })
        .collect();
        said.push(format!("guest_faults={}", count("pgfault")));
        said.push(format!("guest_major_faults={}", count("pgmajfault")));
        // fxdax: what its Fxfs counts of copy-on-write DAX, its private pages among it, if it is
        // mounted and does (/proc/fs/fxfs/<tag>, the first's).
        let stats = fs::read_dir("/proc/fs/fxfs").ok().and_then(|mut dir| dir.find_map(Result::ok));
        let stats = stats.and_then(|stats| fs::read_to_string(stats.path()).ok());
        for line in stats.as_deref().unwrap_or_default().lines() {
            let Some((name, value)) = line.split_once(' ') else { continue };
            if (name.contains("private") || name.contains("cow")) && value.parse::<u64>().is_ok() {
                said.push(format!("guest_fxfs_{name}={value}"));
            }
        }
        said.join(" ")
    }

    /// Says on the console whether the guest's Fxfs's DAX writes copy on write (doc-13): its
    /// `dax_cow` parameter, `on`, `off`, or `absent` from a kernel without it, which
    /// scripts/pgbench-gate.py reports with its case; and fails if the case asked for it
    /// ([`DAX_COW`]) and it is not on.
    pub fn dax_cow() {
        let mode = match fs::read_to_string("/sys/module/fxfs/parameters/dax_cow") {
            Ok(value) if matches!(value.trim(), "Y" | "1") => "on",
            Ok(_) => "off",
            Err(_) => "absent",
        };
        eprintln!("fxfs dax_cow: {mode}");
        if std::env::var_os(DAX_COW).is_some() {
            assert_eq!(mode, "on", "the case asked for fxfs.dax_cow=1");
        }
    }
}

#[guest]
pub use guest::*;
