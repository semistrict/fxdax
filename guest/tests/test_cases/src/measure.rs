// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what a VM costs its host's memory, measured when its guest asks (TASK-164.5). The guest
//! connects to the host over vsock, on [`PORT`], and says `measure PHASE NAME=VALUE...`, its own
//! memory then; the host says, on its standard error, the test's log, the host's memory then, as
//! `memory: vm=VM phase=PHASE NAME=VALUE...`, which scripts/pgbench-gate.py reads, and answers
//! `ok` once the test lets the guest go on. VM 0 is none: the host before the first VM boots.
//! Every value is in bytes, but for counts' (`pages`, `faults`, `total`) and times' (`_ns`, in
//! nanoseconds):
//!
//! - `host_used`: the host's memory not available (/proc/meminfo's MemTotal less MemAvailable):
//!   what the VMs, the daemon and all else hold, each page once, its page cache left out;
//! - `vmm_rss`, `vmm_pss`, `vmm_anon` and `vmm_shmem`: the VM process's resident memory, its
//!   proportional share of it, and of that its anonymous memory (the guest's RAM it touched) and
//!   its shared memory (the daemon's pages its DAX window maps) (/proc/PID/smaps_rollup);
//! - `vmm_maps`: the VM process's mappings (/proc/PID/maps's lines), which its DAX window's runs
//!   take against the host's vm.max_map_count (TASK-227);
//! - `host_kvm_page_tables`: KVM's page tables for its guests, the host's (/proc/meminfo's
//!   SecPageTables), which a DAX window's pages mapped take (TASK-227);
//! - `extentd_pss`: the daemon process's share of its own;
//! - `extentd_*`: some of the daemon's metrics (fxdax::extentd::metrics): the bytes its arenas
//!   hold, every tenant's extents' pages (`extentd_memory_bytes`), the pages the windows map, its
//!   faults, and any gauge of private pages it keeps (copy-on-write DAX's, TASK-164.1);
//! - `guest_*`: what the guest said of itself: its memory (/proc/meminfo), faults (/proc/vmstat),
//!   and its Fxfs's counts of copy-on-write DAX's private pages (`guest_fxfs_*`); fxdax: how long
//!   its kernel took from its start to running init (`guest_boot_ns`, the kernel log's record of
//!   it); the struct pages its kernel made after it booted, its DAX windows' (`guest_memmap`,
//!   /proc/vmstat's nr_memmap_pages); and of its extent devices' DAX windows, their length, how
//!   much of it has struct pages, and the time making them took (`guest_window`,
//!   `guest_window_made`, `guest_window_made_ns`, each device's `window` in sysfs) (TASK-92.8).

use macros::{guest, host};
use std::collections::BTreeMap;

/// The vsock port a guest asks the host to measure it on.
pub const PORT: u32 = 5056;

/// The phase a guest that asks to be measured more than once says last.
pub const LAST: &str = "end";

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
    use crate::vm::Firecracker;
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

    /// The host's memory, and the extent daemon's, of process `daemon`, which serves its metrics at
    /// `metrics`.
    #[derive(Clone, Copy)]
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
            let host = meminfo(&std::fs::read_to_string("/proc/meminfo")?);
            if let Some(tables) = host.get("SecPageTables") {
                write!(line, " host_kvm_page_tables={tables}")?;
            }
            if let Some(pid) = vmm {
                let vmm = rollup(pid).context("the VM process's memory")?;
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
                let maps = std::fs::read_to_string(format!("/proc/{pid}/maps"))
                    .context("the VM process's mappings")?;
                write!(line, " vmm_maps={}", maps.lines().count())?;
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

    /// The vsock of a VM, to give it ([`crate::vm::Vm::vsock`]), on which its guest asks to be
    /// measured: the host's end of [`PORT`].
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
            Request::read(firecracker.accept(&self.listener)?)?
                .context("the guest connected and asked nothing")
        }

        /// Answers every request of the guest of VM `vm`, whose process is `vmm`, on a thread of
        /// its own, saying the host's memory at each through `host`, until [`Served::stop`]: for
        /// a VM whose host side waits on other things as it runs, as one talking to the host on
        /// another port does (TASK-136.1).
        pub fn serve(self, host: HostMemory, vm: usize, vmm: u32) -> Served {
            let path = self.path.clone();
            let thread = std::thread::spawn(move || -> anyhow::Result<()> {
                for stream in self.listener.incoming() {
                    // A connection that asks nothing is [`Served::stop`]'s.
                    let Some(request) = Request::read(stream?)? else { return Ok(()) };
                    host.say(vm, &request.phase, Some(vmm), &request.guest)?;
                    request.answer()?;
                }
                Ok(())
            });
            Served { path, thread }
        }
    }

    /// The requests of a guest a thread answers ([`Requests::serve`]).
    pub struct Served {
        path: PathBuf,
        thread: std::thread::JoinHandle<anyhow::Result<()>>,
    }

    impl Served {
        /// Stops answering, once the requests made are answered, and says whether any failed.
        pub fn stop(self) -> anyhow::Result<()> {
            UnixStream::connect(&self.path)
                .with_context(|| format!("connecting to {}", self.path.display()))?;
            self.thread.join().map_err(|_| anyhow::anyhow!("the measuring thread panicked"))?
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
        /// The request the guest asks on `stream`: none if it asks nothing.
        fn read(stream: UnixStream) -> anyhow::Result<Option<Self>> {
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line)?;
            if line.is_empty() {
                return Ok(None);
            }
            let asked = (line.trim_end().strip_prefix("measure "))
                .with_context(|| format!("the guest asked {line:?}"))?;
            let (phase, guest) = asked.split_once(' ').unwrap_or((asked, ""));
            Ok(Some(Request { phase: phase.to_owned(), guest: guest.to_owned(), stream }))
        }

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
        said.push(format!("guest_memmap={}", count("nr_memmap_pages") * page_size()));
        if let Some(boot) = boot_ns() {
            said.push(format!("guest_boot_ns={boot}"));
        }
        for (name, value) in windows() {
            said.push(format!("guest_{name}={value}"));
        }
        // fxdax: what its Fxfs counts of copy-on-write DAX's private pages, if it is mounted
        // (/proc/fs/fxfs/<tag>, the first's).
        let stats = fs::read_dir("/proc/fs/fxfs").ok().and_then(|mut dir| dir.find_map(Result::ok));
        let stats = stats.and_then(|stats| fs::read_to_string(stats.path()).ok());
        for line in stats.as_deref().unwrap_or_default().lines() {
            let Some((name, value)) = line.split_once(' ') else { continue };
            if name.contains("private") && value.parse::<u64>().is_ok() {
                said.push(format!("guest_fxfs_{name}={value}"));
            }
        }
        said.join(" ")
    }

    fn page_size() -> u64 {
        // SAFETY: A plain system call.
        unsafe { nix::libc::sysconf(nix::libc::_SC_PAGESIZE) as u64 }
    }

    /// How long the kernel took from its start to running init, as its log's record of it says,
    /// if the log still has it: `PRIORITY,SEQUENCE,MICROSECONDS,FLAGS;Run INIT as init process`.
    fn boot_ns() -> Option<u64> {
        crate::kernel_log::records().iter().find_map(|record| {
            let (header, message) = record.split_once(';')?;
            if !(message.starts_with("Run ") && message.trim_end().ends_with(" as init process")) {
                return None;
            }
            Some(header.split(',').nth(2)?.parse::<u64>().ok()? * 1000)
        })
    }

    /// The extent devices' DAX windows, all told: `window`, `window_made` and `window_made_ns`,
    /// each as its devices' `window` attributes say, as lines of `NAME VALUE`.
    fn windows() -> BTreeMap<String, u64> {
        let mut windows = BTreeMap::new();
        let devices = fs::read_dir("/sys/bus/virtio/drivers/virtio_extent").into_iter().flatten();
        for device in devices.flatten() {
            let Ok(window) = fs::read_to_string(device.path().join("window")) else { continue };
            for line in window.lines() {
                let Some((name, value)) = line.split_once(' ') else { continue };
                let value: u64 = value.parse().unwrap_or_else(|_| panic!("window: {line:?}"));
                let name = if name == "len" { "window".to_owned() } else { format!("window_{name}") };
                *windows.entry(name).or_default() += value;
            }
        }
        windows
    }
}

#[guest]
pub use guest::*;
