// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxfs-guest-docker-dax (TASK-50.6): a guest boots on an Fxfs root, a store the host imports from
//! an image holding Docker's static binaries (FXDAX_DOCKER_STATIC names their directory) and an
//! image of a container, a static BusyBox (FXDAX_BUSYBOX), as an OCI archive; its files map from
//! the DAX window. containerd imports the image and runs a container of it, its snapshots overlayfs
//! over Fxfs; then Docker, through that containerd, loads it and runs one, its overlay2 layers on
//! Fxfs too.
//!
//! fxfs-guest-docker-build-dax (TASK-311) builds, as LangSmith's CI builders do: on 8 vCPUs, with
//! BuildKit (Docker's buildx plugin, FXDAX_DOCKER_BUILDX names it), stages in parallel writing
//! FXDAX_DOCKER_BUILD_MIB MiB of files between them (default 256), more than the guest's memory
//! keeps as it builds and exports: the image loaded into Docker's store, saved, and exported as
//! a tar of its files, FXDAX_DOCKER_BUILDS times (default 3), each round's stages anew. Two of 17 such
//! builds oopsed there, then locked up. FXDAX_DOCKER_BUILD_CPUS, a host CPU list such as `0-3`,
//! runs the VM on those CPUs alone: its vCPUs then preempt each other, as on a host with no CPU
//! to spare. Its uploads are some ten times its builds' data (44 GB for ~3.8 GB built, on ubu1):
//! past the default, put the bucket on disk (FXDAX_EXTENTD_REMOTE=file://DIR/), as the default
//! bucket, on /dev/shm, fills at 1024 MiB a build (TASK-314).

use macros::{guest, host};

pub struct TestFxfsGuestDocker {
    /// fxdax: whether the guest builds an image and exports it, rather than runs containers of
    /// one it loads (TASK-311).
    pub build: bool,
}

/// fxdax: the build's VM's vCPUs, as LangSmith's builders have, and its stages, one a vCPU.
const BUILD_VCPUS: u8 = 8;
/// fxdax: the build's settings, from the host's environment, in the guest's.
const BUILDS: &str = "FXDAX_DOCKER_BUILDS";
const BUILD_MIB: &str = "FXDAX_DOCKER_BUILD_MIB";

/// The container image's name, as containerd and Docker name it.
const IMAGE: &str = "docker.io/fxdax/busybox:1";
/// Where the guest finds the image's OCI archive.
const ARCHIVE: &str = "/images/busybox.tar";
/// What the containers say.
#[cfg_attr(feature = "host", allow(dead_code))]
const HELLO: &str = "hello from a container on Fxfs";
/// Where Docker's CLI finds its buildx plugin.
#[cfg_attr(feature = "guest", allow(dead_code))]
const BUILDX: &str = "/usr/libexec/docker/cli-plugins/docker-buildx";

#[host]
pub(crate) mod host {
    use super::*;
    use crate::common::{control, fxdax_guest_should_run};
    use crate::vm::Vm;
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::Context;
    use fxdax::import::testing::{Entry, Kind, layer, oci_layout};
    use std::path::Path;

    const CAPACITY: u64 = 4 << 30;
    const WINDOW: crate::common::Window = crate::common::Window::Large;

    /// Docker's static binaries, as download.docker.com packs them.
    const DOCKER: [&str; 8] = [
        "containerd",
        "containerd-shim-runc-v2",
        "ctr",
        "docker",
        "docker-init",
        "docker-proxy",
        "dockerd",
        "runc",
    ];

    fn read(var: &str, name: &str) -> anyhow::Result<Vec<u8>> {
        let dir = std::env::var_os(var).with_context(|| format!("{var} not set"))?;
        let path = Path::new(&dir).join(name);
        std::fs::read(&path).with_context(|| format!("reading {}", path.display()))
    }

    /// The container's image, BusyBox, as an OCI archive: a tar of its layout, naming it
    /// [`IMAGE`] as containerd and Docker read names.
    fn container_archive(dir: &Path) -> anyhow::Result<Vec<u8>> {
        let busybox = std::env::var_os("FXDAX_BUSYBOX").context("FXDAX_BUSYBOX not set")?;
        let busybox = std::fs::read(busybox)?;
        let rootfs = layer(&[
            Entry::new("bin", Kind::Dir),
            Entry::new("bin/busybox", Kind::File(&busybox)).perm(0o755),
            Entry::new("bin/sh", Kind::Symlink("busybox")),
        ])?;
        let layout = dir.join("busybox-layout");
        oci_layout(&layout, IMAGE, &[rootfs])?;
        // containerd names an image by its own annotation.
        let index_path = layout.join("index.json");
        let mut index: serde_json::Value = serde_json::from_slice(&std::fs::read(&index_path)?)?;
        index["manifests"][0]["annotations"]["io.containerd.image.name"] = IMAGE.into();
        std::fs::write(&index_path, serde_json::to_vec(&index)?)?;
        // Docker's own manifest, as `docker save` writes it beside the layout: Docker's classic
        // storage loads an archive by it alone.
        let blob = |digest: &serde_json::Value| -> anyhow::Result<String> {
            let digest = digest.as_str().context("a digest")?;
            Ok(format!("blobs/sha256/{}", digest.trim_start_matches("sha256:")))
        };
        let manifest_path = layout.join(blob(&index["manifests"][0]["digest"])?);
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest_path)?)?;
        let layers: Vec<String> = (manifest["layers"].as_array().context("layers")?.iter())
            .map(|layer| blob(&layer["digest"]))
            .collect::<anyhow::Result<_>>()?;
        let docker = serde_json::json!([{
            "Config": blob(&manifest["config"]["digest"])?,
            "RepoTags": [IMAGE.trim_start_matches("docker.io/")],
            "Layers": layers,
        }]);
        std::fs::write(layout.join("manifest.json"), serde_json::to_vec(&docker)?)?;
        let mut files = Vec::new();
        for path in ["oci-layout", "index.json", "manifest.json"] {
            files.push((path.to_owned(), std::fs::read(layout.join(path))?));
        }
        for blob in std::fs::read_dir(layout.join("blobs/sha256"))? {
            let blob = blob?;
            let name = format!("blobs/sha256/{}", blob.file_name().to_string_lossy());
            files.push((name, std::fs::read(blob.path())?));
        }
        let mut entries =
            vec![Entry::new("blobs", Kind::Dir), Entry::new("blobs/sha256", Kind::Dir)];
        entries.extend(files.iter().map(|(path, contents)| Entry::new(path, Kind::File(contents))));
        Ok(layer(&entries)?)
    }

    /// fxdax: what a root needs of Docker's, each file's path in the root, its contents and its
    /// permissions, and the directories they are in: Docker's static binaries, in /usr/bin; the
    /// container's archive, at [`ARCHIVE`]; and with `buildx`, Docker's plugin's binary, where
    /// Docker's CLI finds its plugins. fxfs-guest-sandboxes' image has them too, over its own
    /// root's files (TASK-330.1).
    pub(crate) struct DockerFiles {
        pub dirs: Vec<&'static str>,
        pub files: Vec<(String, Vec<u8>, u32)>,
    }

    impl DockerFiles {
        pub(crate) fn new(dir: &Path, buildx: Option<Vec<u8>>) -> anyhow::Result<Self> {
            let mut dirs = vec!["usr", "usr/bin", "images"];
            let mut files: Vec<(String, Vec<u8>, u32)> = (DOCKER.iter())
                .map(|name| {
                    Ok((format!("usr/bin/{name}"), read("FXDAX_DOCKER_STATIC", name)?, 0o755))
                })
                .collect::<anyhow::Result<_>>()?;
            if let Some(buildx) = buildx {
                dirs.extend([
                    "usr/libexec",
                    "usr/libexec/docker",
                    "usr/libexec/docker/cli-plugins",
                ]);
                files.push((BUILDX[1..].to_owned(), buildx, 0o755));
            }
            files.push((ARCHIVE[1..].to_owned(), container_archive(dir)?, 0o644));
            Ok(Self { dirs, files })
        }

        /// Its directories' and files' entries in a layer.
        pub(crate) fn entries(&self) -> Vec<Entry<'_>> {
            let dirs = self.dirs.iter().map(|path| Entry::new(path, Kind::Dir));
            let files = (self.files.iter())
                .map(|(path, contents, perm)| Entry::new(path, Kind::File(contents)).perm(*perm));
            dirs.chain(files).collect()
        }
    }

    /// fxdax: Docker's buildx plugin's binary, FXDAX_DOCKER_BUILDX's (TASK-311).
    pub(crate) fn buildx() -> anyhow::Result<Vec<u8>> {
        let path =
            std::env::var_os("FXDAX_DOCKER_BUILDX").context("FXDAX_DOCKER_BUILDX not set")?;
        std::fs::read(&path).with_context(|| format!("reading {path:?}"))
    }

    /// The root's image: the guest agent, Docker's files, and what init and the daemons mount on
    /// and keep their state in; with `buildx`, Docker's plugin's binary.
    fn root_layer(dir: &Path, buildx: Option<Vec<u8>>) -> anyhow::Result<Vec<u8>> {
        let agent = std::env::var_os("FXDAX_GUEST_AGENT").context("FXDAX_GUEST_AGENT not set")?;
        let agent = std::fs::read(agent)?;
        let docker = DockerFiles::new(dir, buildx)?;
        let mut entries = vec![Entry::new("guest-agent", Kind::File(&agent)).perm(0o755)];
        let dirs = ["dev", "proc", "sys", "run", "etc", "var", "var/lib", "root"];
        entries.extend(dirs.map(|path| Entry::new(path, Kind::Dir)));
        entries.push(Entry::new("tmp", Kind::Dir).perm(0o1777));
        entries.push(Entry::new("var/run", Kind::Symlink("../run")));
        entries.push(Entry::new("etc/passwd", Kind::File(b"root:x:0:0:root:/root:/bin/sh\n")));
        entries.push(Entry::new("etc/group", Kind::File(b"root:x:0:\n")));
        entries.extend(docker.entries());
        Ok(layer(&entries)?)
    }

    /// The build's store: room for its rounds' files, layers and archives.
    const BUILD_CAPACITY: u64 = 16 << 30;

    /// The CPUs of a CPU list as the kernel writes one: `0-3,8,10-11`.
    fn cpu_list(list: &str) -> anyhow::Result<Vec<usize>> {
        let mut cpus = Vec::new();
        for range in list.split(',') {
            let (first, last) = range.split_once('-').unwrap_or((range, range));
            cpus.extend(first.parse::<usize>()?..=last.parse::<usize>()?);
        }
        Ok(cpus)
    }

    impl Test for TestFxfsGuestDocker {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        /// The import of Docker's binaries, and the daemons' starting, take minutes; a build's
        /// rounds take minutes more.
        fn timeout_secs(&self) -> u64 {
            if self.build { 1200 } else { 300 }
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let layout = dir.join("root-layout");
            let buildx = if self.build { Some(buildx()?) } else { None };
            oci_layout(&layout, "fxdax/root:docker", &[root_layer(dir, buildx)?])?;
            // By the daemon, built for release, as fxdaxctl imports: this harness's debug build
            // takes minutes over Docker's binaries.
            let store = dir.join("root");
            let capacity = if self.build { BUILD_CAPACITY } else { CAPACITY };
            let extentd = std::env::var_os("FXDAX_EXTENTD").context("FXDAX_EXTENTD not set")?;
            control(Path::new(&extentd))?.import_image(&store, capacity, &layout, None)?;

            let mut vm = if self.build { Vm::new(BUILD_VCPUS, 2048)? } else { Vm::new(2, 2048)? };
            vm.guest_kernel()?;
            vm.extent_store("root", &store, capacity, crate::common::dax_window(WINDOW)?)?;
            vm.fxfs_root("root");
            if self.build {
                for name in [BUILDS, BUILD_MIB] {
                    if let Ok(value) = std::env::var(name) {
                        vm.env(&std::ffi::CString::new(format!("{name}={value}"))?);
                    }
                }
                if let Ok(cpus) = std::env::var("FXDAX_DOCKER_BUILD_CPUS") {
                    vm.cpus(&cpu_list(&cpus).context("FXDAX_DOCKER_BUILD_CPUS is a CPU list")?);
                }
            }
            vm.enter(&dir.join("rootfs"), &test_setup.test_case)
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8(stdout).unwrap();
            if output == "OK\n" {
                TestOutcome::Pass
            } else {
                TestOutcome::Fail(format!("expected exactly \"OK\\n\", got {output:?}"))
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
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    const CONTAINERD: &str = "/run/containerd/containerd.sock";

    /// Runs `program` with `args`, and returns its standard output; fails with its standard error,
    /// and `logs`', unless it succeeds.
    fn run(program: &str, args: &[&str], logs: &[&str]) -> String {
        let output = Command::new(program).args(args).output().unwrap();
        if !output.status.success() {
            for log in logs {
                eprintln!("{log}:\n{}", std::fs::read_to_string(log).unwrap_or_default());
            }
            panic!(
                "{program} {args:?}: {}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        String::from_utf8(output.stdout).unwrap()
    }

    /// Starts the daemon `program` with `args`, logging to `log`, and waits for its socket.
    fn start(program: &str, args: &[&str], log: &str, socket: &str) -> Child {
        let daemon = Command::new(program)
            .args(args)
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(log).unwrap())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        while !Path::new(socket).exists() {
            if Instant::now() > deadline {
                panic!("{program} never listened: {}", std::fs::read_to_string(log).unwrap());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        daemon
    }

    fn mount_run() {
        let (source, target) = (CString::new("tmpfs").unwrap(), CString::new("/run").unwrap());
        let fstype = CString::new("tmpfs").unwrap();
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount /run: {}", std::io::Error::last_os_error());
    }

    /// A setting of the build's, `name`, from the guest's environment, or `default`.
    fn setting(name: &str, default: u64) -> u64 {
        match std::env::var(name) {
            Ok(value) => value.parse().unwrap_or_else(|_| panic!("{name}={value}: a count")),
            Err(_) => default,
        }
    }

    /// The build's Dockerfile: `stages` stages from BusyBox's image, run at once, each writing
    /// `files` files of `kib` KiB of random bytes, the round's, then a gzipped tar of them, each
    /// step a container, and a cgroup, of its own; the image their files together, and the round
    /// it was built in, which makes every round's stages anew (ROUND, a build argument).
    fn dockerfile(stages: u64, files: u64, kib: u64) -> String {
        let busybox = "/bin/busybox";
        let mut dockerfile = format!("FROM {IMAGE} AS base\n");
        for stage in 0..stages {
            dockerfile += &format!(
                "FROM base AS s{stage}\n\
                 ARG ROUND\n\
                 RUN --network=none {busybox} mkdir /out && i=0 && while [ $i -lt {files} ]; do \
                 {busybox} dd if=/dev/urandom of=/out/f$i bs=1024 count={kib} 2>/dev/null || exit 1; \
                 i=$((i+1)); done && echo $ROUND > /out/round\n\
                 RUN --network=none {busybox} tar -c /out | {busybox} gzip -1 > /out.tgz\n"
            );
        }
        dockerfile += "FROM base\nARG ROUND\n";
        for stage in 0..stages {
            dockerfile += &format!("COPY --from=s{stage} /out /s{stage}\n");
            dockerfile += &format!("COPY --from=s{stage} /out.tgz /s{stage}.tgz\n");
        }
        dockerfile += "RUN --network=none echo $ROUND > /built\n";
        dockerfile
    }

    /// Builds the image FXDAX_DOCKER_BUILDS times, through dockerd's BuildKit, and exports it each
    /// time: loaded into Docker's store, saved, and as a tar of its files; then runs it, which
    /// says the round it was built in, and lets go of the round's images, archives and cache.
    fn build(logs: &[&str]) {
        let stages = u64::from(BUILD_VCPUS);
        let kib_per_stage = setting(BUILD_MIB, 256) * 1024 / stages;
        const FILE_KIB: u64 = 64;
        std::fs::create_dir("/build").unwrap();
        let dockerfile = dockerfile(stages, kib_per_stage / FILE_KIB, FILE_KIB);
        std::fs::write("/build/Dockerfile", dockerfile).unwrap();
        for round in 1..=setting(BUILDS, 3) {
            let began = Instant::now();
            let tag = format!("fxdax/built:{round}");
            let arg = format!("ROUND={round}");
            let build = ["buildx", "build", "--progress=plain", "--build-arg", &arg];
            let mut load = build.to_vec();
            load.extend(["--tag", &tag, "--load", "/build"]);
            run("docker", &load, logs);
            run("docker", &["save", "--output", "/tmp/built.tar", &tag], logs);
            let mut export = build.to_vec();
            export.extend(["--output", "type=tar,dest=/tmp/built-files.tar", "/build"]);
            run("docker", &export, logs);
            let said = run(
                "docker",
                &["run", "--rm", "--network", "none", &tag, "/bin/busybox", "cat", "/built"],
                logs,
            );
            assert_eq!(said, format!("{round}\n"), "the image of round {round}");
            run("docker", &["image", "rm", &tag], logs);
            run("docker", &["buildx", "prune", "--all", "--force"], logs);
            for archive in ["/tmp/built.tar", "/tmp/built-files.tar"] {
                std::fs::remove_file(archive).unwrap();
            }
            eprintln!("docker build: round {round} in {:.1} s", began.elapsed().as_secs_f64());
        }
        // What the builds asked of reclaim, and of the shadow nodes' list_lru LangSmith's
        // locked-up vCPU spun in (workingset_update_node).
        let vmstat = std::fs::read_to_string("/proc/vmstat").unwrap();
        let counted = [
            "workingset_nodes",
            "workingset_refault_file",
            "workingset_nodereclaim",
            "pgscan_kswapd",
            "pgscan_direct",
            "pgsteal_kswapd",
            "pgsteal_direct",
        ];
        for line in vmstat.lines().filter(|line| counted.contains(&line.split(' ').next().unwrap()))
        {
            eprintln!("docker build: vmstat {line}");
        }
    }

    impl Test for TestFxfsGuestDocker {
        fn in_guest(self: Box<Self>) {
            // The root is Fxfs, and the daemons keep their state on it.
            let mounts = std::fs::read_to_string("/proc/mounts").unwrap();
            let root = mounts.lines().find(|line| line.split(' ').nth(1) == Some("/")).unwrap();
            assert_eq!(root.split(' ').nth(2), Some("fxfs"), "{root}");
            mount_run();

            let containerd_log = "/tmp/containerd.log";
            let _containerd = start(
                "containerd",
                &[
                    "--address",
                    CONTAINERD,
                    "--root",
                    "/var/lib/containerd",
                    "--state",
                    "/run/containerd",
                ],
                containerd_log,
                CONTAINERD,
            );
            let ctr = |args: &[&str]| {
                let mut all = vec!["--address", CONTAINERD];
                all.extend(args);
                run("ctr", &all, &[containerd_log])
            };
            if !self.build {
                ctr(&["images", "import", ARCHIVE]);
                let said = ctr(&["run", "--rm", IMAGE, "hello", "/bin/busybox", "echo", HELLO]);
                assert_eq!(said, format!("{HELLO}\n"), "containerd's container");
            }

            let dockerd_log = "/tmp/dockerd.log";
            let mut dockerd = vec![
                "--containerd",
                CONTAINERD,
                "--data-root",
                "/var/lib/docker",
                "--iptables=false",
                "--ip6tables=false",
                "--bridge=none",
            ];
            // A build's in Docker's own default store, as a builder's is.
            if !self.build {
                dockerd.extend(["--storage-driver", "overlay2"]);
            }
            let _dockerd = start("dockerd", &dockerd, dockerd_log, "/run/docker.sock");
            let logs = [dockerd_log, containerd_log];
            let loaded = run("docker", &["load", "--input", ARCHIVE], &logs);
            if self.build {
                build(&logs);
            } else {
                // "Loaded image: NAME", or "Loaded image ID: ID".
                let image =
                    loaded.lines().filter_map(|line| line.split(": ").nth(1)).last().unwrap();
                let said = run(
                    "docker",
                    &["run", "--rm", "--network", "none", image, "/bin/busybox", "echo", HELLO],
                    &logs,
                );
                assert_eq!(said, format!("{HELLO}\n"), "Docker's container");
                let driver = run("docker", &["info", "--format", "{{.Driver}}"], &logs);
                assert_eq!(driver, "overlay2\n");
            }
            // overlayfs's work over Fxfs left it consistent (TASK-50.6.1).
            let inconsistent: Vec<String> = crate::kernel_log::records()
                .into_iter()
                .filter(|record| record.contains("Filesystem inconsistency"))
                .collect();
            assert_eq!(inconsistent, Vec::<String>::new());
            println!("OK");
        }
    }
}
