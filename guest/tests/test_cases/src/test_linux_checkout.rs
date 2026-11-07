// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What a checkout the size of Linux's costs (fxdax's TASK-136.1, backlog doc-10): on the guest's
//! Fxfs, with a DAX window and without, against ext4 on a virtio-blk disk, and on virtio-pmem with
//! DAX, which TASK-227 takes Fxfs's DAX to match; and as a shared volume, a sandbox's volume over a
//! read-only base.
//!
//! A first VM checks Linux out: it clones it, without a checkout, from the bare repo the root
//! filesystem holds, then checks it out, and syncs. Then, its caches dropped, it runs `git status`
//! cold and warm, and once `sync` has written what that left, such as directories' access times;
//! and reads the tree whole, every file of the checkout but `.git`'s, cold and warm. A second VM,
//! booted on the same disk or store, does the same, a guest that has seen nothing of it. On ext4,
//! that is all.
//!
//! On Fxfs, the first VM checks Linux out into a volume and publishes it. Then, in fresh VMs, each
//! on a store of its own, the guest measures what a sandbox pays for the shared volume:
//! `SANDBOXES` VMs in turn attach the generation and mount it alone, run the same statuses and
//! reads, the first `git status` lstatting every tracked file and hashing none, and write to a file
//! of the base and a new one; and a last one mounts it `REPOS` times over, as volumes of its own,
//! and boots again with them closed. The host measures the bytes of the volume's layers and its
//! records in the allocator's tree, which are what baking the checkout into a store would cost
//! every mount (doc-10), and what each store and bucket takes on its disk after each VM.
//!
//! Every VM is measured as its guest asks ([`crate::measure`]): the host's memory, the VM
//! process's and the extent daemon's, which the case starts for itself, and the guest's, on the
//! log's `memory:` lines. The guest's timings are lines of standard output, `measure ROLE NAME
//! VALUE UNIT`, which the log keeps; a memory stage whose name ends in `-pinned` is measured after
//! dropping the guest's caches. In no suite, for what it takes: each case runs by name
//! (`FXDAX_E2E_TEST=linux-checkout-fxfs-dax scripts/test-gce.sh`), and
//! scripts/linux-checkout-report.py makes tables of what they said. FXDAX_LINUX_CHECKOUT_WINDOW
//! sets the DAX window's bytes (as large as the data, [`crate::common::dax_window`]), and
//! FXDAX_LINUX_CHECKOUT_EXTENTD_MEMORY the daemon's ([`crate::common::DAEMON_MEMORY`]): each
//! larger than the tree, its warm reread finds it all still there, as ext4's finds the host's page
//! cache; a window or memory smaller than the tree, 1 GiB, finds nothing of it the second time
//! (GCE r870 and r881: 25.6 s against 600 ms, TASK-227). ext4's disk, and its pmem device, is an 8 GiB image file, the host's page cache
//! holding what it has of it: a virtio-pmem device maps the file into the guest whole.

use macros::{guest, host};

/// The filesystem a case checks Linux out on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checkout {
    /// ext4 on a virtio-blk disk, on a raw image file the host's page cache caches.
    Ext4,
    /// ext4 with DAX (`dax=always`) on writable virtio-pmem, on a raw image file the VM maps:
    /// files read in place from the host's page cache, TASK-227's mark for Fxfs's DAX.
    Ext4Pmem,
    /// The guest's Fxfs on an extent-store device, with files in the page cache.
    Fxfs,
    /// The guest's Fxfs on an extent-store device, mapping files from its DAX window.
    FxfsDax,
}

impl Checkout {
    /// Whether it is ext4's, on an image file: one VM checks it out, another opens it, and no
    /// volume is shared.
    fn ext4(self) -> bool {
        matches!(self, Checkout::Ext4 | Checkout::Ext4Pmem)
    }
}

pub struct TestLinuxCheckout {
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub on: Checkout,
}

/// The variable naming the ext4 device in the guest.
#[cfg_attr(feature = "host", allow(dead_code))]
const DEVICE: &str = "FXDAX_LINUX_CHECKOUT_DEVICE";

#[host]
mod host {
    use super::*;
    use crate::common::{
        Window, control, dax_window, fxdax_guest_should_run, setup_rootfs, spawn_daemon,
    };
    use crate::measure::{HostMemory, metrics_address};
    use crate::shared_volumes::host::{Vms, format, next_generation};
    use crate::vm::{Vm, without_kernel_lines};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::Context;
    use futures::executor::block_on;
    use fxdax::{Image, Storage};
    use std::ffi::{CString, OsStr};
    use std::os::unix::fs::MetadataExt as _;
    use std::path::Path;

    /// Linux, its tree alone, which the first VM clones, and Git, strace, for the first `git
    /// status`'s check, and e2fsprogs, for ext4's. The clone's objects are in one pack, as a repo
    /// cloned whole has; its history isn't what a checkout's costs depend on.
    const CONTAINERFILE: &str = "FROM fedora:43
RUN dnf install -y git strace e2fsprogs && dnf clean all
RUN git clone --bare --depth 1 --branch v6.18 https://github.com/torvalds/linux /linux.git
";
    /// The VMs that mount the shared checkout alone, one after the other.
    const SANDBOXES: usize = 3;
    const CPUS: u8 = 4;
    /// Enough for what `REPOS` volumes take, which the test measures.
    const MEMORY_MIB: u32 = 4096;
    const CAPACITY: u64 = 8 << 30;

    /// The bytes the variable `name` says, if set, as a count.
    fn bytes(name: &str) -> anyhow::Result<Option<u64>> {
        let Some(value) = std::env::var(name).ok().filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        Ok(Some(value.parse().with_context(|| format!("{name} is a count of bytes"))?))
    }

    /// Says what the volume of the first VM's store at `path` takes of its metadata.
    fn measure_metadata(path: &Path) -> anyhow::Result<()> {
        block_on(async {
            let image = Image::mount(Storage::open(path)?).await?;
            let fs = image.open_volume("repo").await?;
            let metadata = fs.metadata().await?;
            println!("measure host volume-layers {} bytes", metadata.layer_bytes);
            println!("measure host volume-allocator-records {} records", metadata.allocator_records);
            fs.unmount().await?;
            image.close().await?;
            Ok(())
        })
    }

    /// The bytes of the host's disk that what `path` names takes, all it holds told.
    fn disk_bytes(path: &Path) -> std::io::Result<u64> {
        let metadata = std::fs::symlink_metadata(path)?;
        let mut bytes = metadata.blocks() * 512;
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path)? {
                bytes += disk_bytes(&entry?.path())?;
            }
        }
        Ok(bytes)
    }

    /// Says what each of the stores, buckets and images in `dir` takes of the host's disk, after
    /// the VM of `role` ran.
    fn measure_disk(dir: &Path, role: &str) -> anyhow::Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            let bytes = disk_bytes(&entry.path())?;
            println!("measure host {role}-disk-{} {bytes} bytes", name.to_string_lossy());
        }
        Ok(())
    }

    impl TestLinuxCheckout {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let root = setup_rootfs(test_setup)?;
            // A daemon of the case's own, whose memory the host measures, its socket's path short,
            // as Unix sockets' must be; its bucket a directory, whose bytes the host measures.
            let sockets = tempdir::TempDir::new("fxdax-linux")?;
            let extentd = sockets.path().join("extentd.sock");
            let metrics = metrics_address()?;
            let bucket = dir.join("bucket");
            std::fs::create_dir(&bucket)?;
            let (metrics_at, bucket_at) =
                (metrics.to_string(), format!("file://{}/", bucket.canonicalize()?.display()));
            let env = [
                ("FXDAX_EXTENTD_METRICS", OsStr::new(&metrics_at)),
                ("FXDAX_EXTENTD_REMOTE", OsStr::new(&bucket_at)),
            ];
            let memory = bytes("FXDAX_LINUX_CHECKOUT_EXTENTD_MEMORY")?;
            let daemon = spawn_daemon(test_setup, &extentd, "extentd", &env, memory)?;
            let memory = HostMemory::new(daemon.id(), metrics);
            memory.say(0, "before", None, "")?;
            let window = match self.on {
                Checkout::FxfsDax => match bytes("FXDAX_LINUX_CHECKOUT_WINDOW")? {
                    Some(window) => window,
                    None => dax_window(Window::Large)?,
                },
                Checkout::Ext4 | Checkout::Ext4Pmem | Checkout::Fxfs => 0,
            };
            let mut vms = Vms::new(test_setup, root, extentd, CAPACITY, window);
            (vms.cpus, vms.memory_mib, vms.memory) = (CPUS, MEMORY_MIB, Some(memory));
            match self.on {
                Checkout::Ext4 | Checkout::Ext4Pmem => ext4(&vms, dir, self.on),
                Checkout::Fxfs | Checkout::FxfsDax => fxfs(&vms, dir),
            }
        }
    }

    /// The checkout on ext4, `on` a disk or a pmem device the two VMs boot with.
    fn ext4(vms: &Vms, dir: &Path, on: Checkout) -> anyhow::Result<()> {
        let image = dir.join("ext4.img");
        std::fs::File::create(&image)?.set_len(CAPACITY)?;
        let disk = |vm: &mut Vm| -> anyhow::Result<()> {
            let device = match on {
                Checkout::Ext4 => vm.disk("repo", &image)?,
                Checkout::Ext4Pmem => vm.pmem("repo", &image)?,
                Checkout::Fxfs | Checkout::FxfsDax => anyhow::bail!("{on:?} is no ext4"),
            };
            vm.env(&CString::new(format!("{DEVICE}={device}"))?);
            Ok(())
        };
        vms.boot("checkout", false, disk)?.0.wait()?;
        measure_disk(dir, "checkout")?;
        vms.boot("reopened", false, disk)?.0.wait()?;
        measure_disk(dir, "reopened")
    }

    /// The checkout on Fxfs: in the first VM's store, which a second VM opens, and published, which
    /// sandboxes attach.
    fn fxfs(vms: &Vms, dir: &Path) -> anyhow::Result<()> {
        let control = control(&vms.extentd)?;
        let publisher = dir.join("publisher");
        format(&publisher, CAPACITY)?;
        let (vm, stream) = vms.run("checkout", &publisher, true)?;
        let mut stream = stream.context("the first VM's connection")?;
        let path = dir.join("generation");
        let generation = next_generation(&mut stream, &control, &publisher, path)?;
        vm.wait()?;
        measure_metadata(&publisher)?;
        measure_disk(dir, "checkout")?;
        vms.run("reopened", &publisher, false)?.0.wait()?;

        let generations = [generation];
        for sandbox in 0..SANDBOXES {
            let role = format!("sandbox-{sandbox}");
            vms.run_consumer(&role, &dir.join(&role), &control, &generations)?;
            measure_disk(dir, &role)?;
        }
        let store = dir.join("hundreds");
        vms.run_consumer("hundreds", &store, &control, &generations)?;
        measure_disk(dir, "hundreds")?;
        vms.run("rebooted", &store, false)?.0.wait()
    }

    impl Test for TestLinuxCheckout {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn rootfs_image(&self) -> Option<&'static str> {
            Some(CONTAINERFILE)
        }

        /// The clone alone took 8 to 44 minutes in Lima on a busy host.
        fn timeout_secs(&self) -> u64 {
            7200
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            if let Err(err) = self.run(&test_setup) {
                eprintln!("{err:#}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
            let output = without_kernel_lines(&String::from_utf8(stdout).unwrap());
            let said: Vec<_> = output.lines().filter(|line| line.starts_with("OK ")).collect();
            let roles: Vec<String> = if self.on.ext4() {
                vec!["checkout".into(), "reopened".into()]
            } else {
                ["checkout", "reopened"]
                    .into_iter()
                    .map(str::to_owned)
                    .chain((0..SANDBOXES).map(|sandbox| format!("sandbox-{sandbox}")))
                    .chain(["hundreds".to_owned(), "rebooted".to_owned()])
                    .collect()
            };
            let expected: Vec<_> = roles.iter().map(|role| format!("OK {role}")).collect();
            if said == expected {
                TestOutcome::Pass
            } else {
                TestOutcome::Fail(format!("expected {expected:?}, got {said:?}"))
            }
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;

    use crate::shared_volumes::guest::{
        HOME, MNT, REPO, attach_generation, check_first_status, connect, drop_caches, git, give,
        mount, mount_repo, publish, settle_index, sync, tracked_files, try_import, unmount,
    };
    use std::fs::{self, OpenOptions};
    use std::io::Read as _;
    use std::os::unix::fs::{FileExt as _, MetadataExt as _};
    use std::path::Path;
    use std::process::Command;
    use std::time::{Duration, Instant};

    /// Linux's bare clone, which the root filesystem's image holds.
    const UPSTREAM: &str = "/linux.git";
    /// How many volumes the last VM mounts at once.
    const REPOS: usize = 256;
    /// A file of the base, which the first write changes.
    const OVERWRITTEN: &str = "MAINTAINERS";
    /// A file the first write makes.
    const CREATED: &str = "new-file";
    /// What `git status` finds after those writes.
    const WRITTEN: &str = " M MAINTAINERS\n?? new-file\n";

    /// Says what the guest of a role measures.
    struct Measure {
        role: String,
    }

    impl Measure {
        fn say(&self, name: &str, value: impl std::fmt::Display, unit: &str) {
            println!("measure {} {name} {value} {unit}", self.role);
        }

        fn duration(&self, name: &str, duration: Duration) {
            self.say(name, duration.as_secs_f64() * 1e3, "ms");
        }

        /// Runs `f`, saying how long it took as `name`.
        fn time<T>(&self, name: &str, f: impl FnOnce() -> T) -> T {
            let start = Instant::now();
            let output = f();
            self.duration(name, start.elapsed());
            output
        }

        /// Says how long `durations` took: each of many of one thing.
        fn spread(&self, name: &str, durations: &[Duration]) {
            let mut sorted = durations.to_vec();
            sorted.sort_unstable();
            self.duration(&format!("{name}-first"), durations[0]);
            self.duration(&format!("{name}-median"), sorted[sorted.len() / 2]);
            self.duration(&format!("{name}-max"), sorted[sorted.len() - 1]);
            self.duration(&format!("{name}-total"), durations.iter().sum());
        }

        /// Has the host measure its memory and the guest's as of `stage` ([`crate::measure`]).
        fn memory(&self, stage: &str) {
            crate::measure::measure(&format!("{}-{stage}", self.role));
        }

        /// The same, once the guest's caches are dropped: what it keeps.
        fn pinned(&self, stage: &str) {
            drop_caches();
            self.memory(&format!("{stage}-pinned"));
        }
    }

    /// How long `f` takes.
    fn elapsed(f: impl FnOnce()) -> Duration {
        let start = Instant::now();
        f();
        start.elapsed()
    }

    /// Runs `program` with `args`, as root, which must succeed.
    fn run(program: &str, args: &[&str]) {
        let status = Command::new(program).args(args).status();
        let status = status.unwrap_or_else(|error| panic!("{program}: {error}"));
        assert!(status.success(), "{program} {args:?}: {status}");
    }

    /// Adds what the directory `path` holds to `files`, `directories` and `bytes`.
    fn walk(path: &Path, files: &mut u64, directories: &mut u64, bytes: &mut u64) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let metadata = fs::symlink_metadata(entry.path()).unwrap();
            if metadata.is_dir() {
                *directories += 1;
                walk(&entry.path(), files, directories, bytes);
            } else {
                *files += 1;
                *bytes += metadata.size();
            }
        }
    }

    /// Says what the tree at `path` holds, as `name`.
    fn measure_tree(measure: &Measure, name: &str, path: &str) {
        let (mut files, mut directories, mut bytes) = (0, 0, 0);
        walk(Path::new(path), &mut files, &mut directories, &mut bytes);
        measure.say(&format!("{name}-files"), files, "files");
        measure.say(&format!("{name}-directories"), directories, "directories");
        measure.say(&format!("{name}-bytes"), bytes, "bytes");
    }

    /// Reads every regular file under `path` whole, but `.git`'s, through `buffer`, and returns how
    /// many bytes they held.
    fn read_tree(path: &Path, buffer: &mut [u8]) -> u64 {
        let mut bytes = 0;
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() && entry.file_name() != ".git" {
                bytes += read_tree(&entry.path(), buffer);
            } else if kind.is_file() {
                let mut file = fs::File::open(entry.path()).unwrap();
                loop {
                    let read = file.read(buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    bytes += read as u64;
                }
            }
        }
        bytes
    }

    /// Reads the checkout at `path` whole, its caches dropped, then again.
    fn reads(measure: &Measure, path: &str) {
        let mut buffer = vec![0; 1 << 20];
        drop_caches();
        let bytes = measure.time("read-cold", || read_tree(Path::new(path), &mut buffer));
        measure.say("read-bytes", bytes, "bytes");
        assert_eq!(measure.time("read-warm", || read_tree(Path::new(path), &mut buffer)), bytes);
    }

    /// The `git status`es of the repo at `path`, which find it clean: the first, its caches dropped,
    /// the next, the first once `sync` has written what they left, such as directories' access
    /// times, and the one after.
    fn statuses(measure: &Measure, path: &str) {
        let status = ["status", "--porcelain"];
        // What a program costs to load isn't what this measures.
        git(path, &["--version"]);
        drop_caches();
        assert_eq!(measure.time("status-cold", || git(path, &status)), "");
        measure.memory("status");
        assert_eq!(measure.time("status-warm", || git(path, &status)), "");
        sync();
        assert_eq!(measure.time("status-synced", || git(path, &status)), "");
        assert_eq!(measure.time("status-warm-again", || git(path, &status)), "");
    }

    /// Checks Linux out at `REPO`, measures it, and publishes it if it is a volume.
    fn checkout(measure: &Measure, on: Checkout) {
        if !on.ext4() {
            mount_repo();
        }
        measure.pinned("booted");
        git("/", &["--version"]);
        let clone =
            ["-c", "safe.directory=*", "clone", "--quiet", "--no-local", "--no-checkout", UPSTREAM];
        measure.time("clone", || git("/", &[&clone[..], &[REPO]].concat()));
        git(REPO, &["config", "gc.auto", "0"]);
        measure.time("checkout", || git(REPO, &["reset", "--hard", "--quiet", "HEAD"]));
        measure.time("sync", sync);
        measure.say("tracked-files", tracked_files(REPO).len(), "files");
        measure_tree(measure, "volume", REPO);
        measure_tree(measure, "git", &format!("{REPO}/.git"));
        settle_index();
        sync();
        measure.memory("checked-out");
        measure.pinned("checked-out");

        statuses(measure, REPO);
        reads(measure, REPO);
        measure.memory("read");
        measure.pinned("read");
        if !on.ext4() {
            let mut host = connect();
            let manifest = measure.time("publish", || publish(&mut host));
            measure.say("manifest", manifest, "bytes");
        }
        unmount(REPO);
    }

    /// Opens the checkout at `REPO` in a VM that has seen nothing of it, and measures it.
    fn reopened(measure: &Measure, on: Checkout) {
        measure.pinned("booted");
        if !on.ext4() {
            // Given to the user as the first VM checked it out: nothing to write.
            measure.time("mount", || mount(Some("repo"), REPO));
        }
        measure.memory("mounted");
        statuses(measure, REPO);
        reads(measure, REPO);
        measure.memory("read");
        measure.pinned("read");
        unmount(REPO);
    }

    /// The first write to the repo at `path`, each to the device: to a file of the base, and a new
    /// file.
    fn write(measure: &Measure, path: &str) {
        let overwritten = format!("{path}/{OVERWRITTEN}");
        measure.time("first-overwrite", || {
            let file = OpenOptions::new().write(true).open(&overwritten).unwrap();
            file.write_all_at(b"#", 0).unwrap();
            file.sync_all().unwrap();
        });
        let created = format!("{path}/{CREATED}");
        measure.time("first-create", || {
            let file = fs::File::create_new(&created).unwrap();
            file.write_all_at(b"the sandbox's\n", 0).unwrap();
            file.sync_all().unwrap();
        });
    }

    fn sandbox(measure: &Measure) {
        measure.pinned("booted");
        let mut host = connect();
        let manifest = measure.time("attach", || attach_generation(&mut host, 0));
        measure.time("import", || try_import("linux", &manifest).unwrap());
        let path = format!("{MNT}/linux");
        fs::create_dir(&path).unwrap();
        measure.time("mount", || mount(Some("linux"), &path));
        measure.memory("mounted");

        statuses(measure, &path);
        check_first_status(&path);
        reads(measure, &path);
        measure.memory("read");
        measure.pinned("read");

        write(measure, &path);
        assert_eq!(
            measure.time("status-after-writes", || git(&path, &["status", "--porcelain"])),
            WRITTEN
        );
        measure.pinned("written");
        unmount(&path);
    }

    fn repo(number: usize) -> String {
        format!("{MNT}/repo-{number}")
    }

    fn volume(number: usize) -> String {
        format!("linux-{number}")
    }

    /// Unmounts the `REPOS` volumes.
    fn unmount_all(measure: &Measure) {
        measure.time("unmount-all", || {
            for number in 0..REPOS {
                unmount(&repo(number));
            }
        });
    }

    fn hundreds(measure: &Measure) {
        measure.pinned("booted");
        let mut host = connect();
        let manifest = attach_generation(&mut host, 0);
        let (mut imports, mut mounts) = (Vec::new(), Vec::new());
        for number in 0..REPOS {
            let (path, name) = (repo(number), volume(number));
            fs::create_dir(&path).unwrap();
            imports.push(elapsed(|| try_import(&name, &manifest).unwrap()));
            mounts.push(elapsed(|| mount(Some(&name), &path)));
        }
        measure.spread("import", &imports);
        measure.spread("mount", &mounts);
        measure.pinned("mounted");

        statuses(measure, &repo(0));
        write(measure, &repo(0));
        measure.pinned("written");
        unmount_all(measure);
    }

    /// `hundreds`, booted again, with the volumes closed: what opens is only what mounts.
    fn rebooted(measure: &Measure) {
        measure.pinned("booted");
        measure.time("mount", || mount(Some(&volume(1)), &repo(1)));
        measure.memory("mounted");
        statuses(measure, &repo(1));
        measure.pinned("status");

        let others = (0..REPOS).filter(|number| *number != 1);
        let mounts: Vec<_> =
            others.map(|number| elapsed(|| mount(Some(&volume(number)), &repo(number)))).collect();
        measure.spread("mount-others", &mounts);
        measure.pinned("mounted-all");
        unmount_all(measure);
    }

    impl Test for TestLinuxCheckout {
        fn in_guest(self: Box<Self>) {
            let role = std::env::var("FXDAX_SHARED_VOLUMES").unwrap();
            let measure = Measure { role: role.clone() };
            fs::create_dir_all(MNT).unwrap();
            if self.on.ext4() {
                let device = std::env::var(DEVICE).unwrap();
                let fresh = role == "checkout";
                if fresh {
                    measure.time("mkfs", || run("mkfs.ext4", &["-q", "-F", &device]));
                }
                fs::create_dir_all(REPO).unwrap();
                // On pmem, ext4 maps files from the device, or refuses to mount.
                let options = if self.on == Checkout::Ext4Pmem { "dax=always" } else { "defaults" };
                let mount = ["-t", "ext4", "-o", options, &device, REPO];
                measure.time("mount", || run("mount", &mount));
                if self.on == Checkout::Ext4Pmem {
                    let mounts = fs::read_to_string("/proc/mounts").unwrap();
                    let line = mounts.lines().find(|line| line.contains(REPO)).unwrap();
                    assert!(line.contains("dax=always"), "ext4 mounted without DAX: {line}");
                }
                if fresh {
                    // Git clones into an empty directory alone.
                    fs::remove_dir(format!("{REPO}/lost+found")).unwrap();
                    give(REPO);
                }
            } else {
                measure.time("default-mount", || mount(None, MNT));
            }
            give(HOME);
            match (self.on, role.split('-').next().unwrap()) {
                (on, "checkout") => checkout(&measure, on),
                (on, "reopened") => reopened(&measure, on),
                (on, "sandbox") if !on.ext4() => sandbox(&measure),
                (on, "hundreds") if !on.ext4() => hundreds(&measure),
                (on, "rebooted") if !on.ext4() => rebooted(&measure),
                (on, role) => panic!("no role {role} on {on:?}"),
            }
            if !self.on.ext4() {
                unmount(MNT);
            }
            println!("OK {role}");
        }
    }
}
