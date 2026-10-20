// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxfs-guest-docker-dax (TASK-50.6): a guest boots on an Fxfs root, a store the host imports from
//! an image holding Docker's static binaries (FXDAX_DOCKER_STATIC names their directory) and an
//! image of a container, a static BusyBox (FXDAX_BUSYBOX), as an OCI archive; its files map from
//! the DAX window. containerd imports the image and runs a container of it, its snapshots overlayfs
//! over Fxfs; then Docker, through that containerd, loads it and runs one, its overlay2 layers on
//! Fxfs too.

use macros::{guest, host};

pub struct TestFxfsGuestDocker;

/// The container image's name, as containerd and Docker name it.
const IMAGE: &str = "docker.io/fxdax/busybox:1";
/// Where the guest finds the image's OCI archive.
const ARCHIVE: &str = "/images/busybox.tar";
/// What the containers say.
#[cfg_attr(feature = "host", allow(dead_code))]
const HELLO: &str = "hello from a container on Fxfs";

#[host]
mod host {
    use super::*;
    use crate::common::{control, fxdax_guest_should_run};
    use crate::vm::Vm;
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::Context;
    use fxdax::import::testing::{Entry, Kind, layer, oci_layout};
    use std::path::Path;

    const CAPACITY: u64 = 4 << 30;
    const WINDOW: u64 = 1 << 30;

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

    /// The root's image: the guest agent, Docker's binaries, the container's archive, and what
    /// init and the daemons mount on and keep their state in.
    fn root_layer(dir: &Path) -> anyhow::Result<Vec<u8>> {
        let agent = std::env::var_os("FXDAX_GUEST_AGENT").context("FXDAX_GUEST_AGENT not set")?;
        let agent = std::fs::read(agent)?;
        let binaries: Vec<(String, Vec<u8>)> = (DOCKER.iter())
            .map(|name| Ok((format!("usr/bin/{name}"), read("FXDAX_DOCKER_STATIC", name)?)))
            .collect::<anyhow::Result<_>>()?;
        let archive = container_archive(dir)?;
        let mut entries = vec![Entry::new("guest-agent", Kind::File(&agent)).perm(0o755)];
        let dirs =
            ["dev", "proc", "sys", "run", "etc", "usr", "usr/bin", "var", "var/lib", "images"];
        entries.extend(dirs.map(|path| Entry::new(path, Kind::Dir)));
        entries.push(Entry::new("tmp", Kind::Dir).perm(0o1777));
        entries.push(Entry::new("var/run", Kind::Symlink("../run")));
        entries.push(Entry::new("etc/passwd", Kind::File(b"root:x:0:0:root:/root:/bin/sh\n")));
        entries.push(Entry::new("etc/group", Kind::File(b"root:x:0:\n")));
        for (path, contents) in &binaries {
            entries.push(Entry::new(path, Kind::File(contents)).perm(0o755));
        }
        entries.push(Entry::new(&ARCHIVE[1..], Kind::File(&archive)));
        Ok(layer(&entries)?)
    }

    impl Test for TestFxfsGuestDocker {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        /// The import of Docker's binaries, and the daemons' starting, take minutes.
        fn timeout_secs(&self) -> u64 {
            300
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let layout = dir.join("root-layout");
            oci_layout(&layout, "fxdax/root:docker", &[root_layer(dir)?])?;
            // By the daemon, built for release, as fxdaxctl imports: this harness's debug build
            // takes minutes over Docker's binaries.
            let store = dir.join("root");
            let extentd = std::env::var_os("FXDAX_EXTENTD").context("FXDAX_EXTENTD not set")?;
            control(Path::new(&extentd))?.import_image(&store, CAPACITY, &layout, None)?;

            let mut vm = Vm::new(2, 2048)?;
            vm.guest_kernel()?;
            vm.extent_store("root", &store, CAPACITY, WINDOW)?;
            vm.fxfs_root("root");
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
            ctr(&["images", "import", ARCHIVE]);
            let said = ctr(&["run", "--rm", IMAGE, "hello", "/bin/busybox", "echo", HELLO]);
            assert_eq!(said, format!("{HELLO}\n"), "containerd's container");

            let dockerd_log = "/tmp/dockerd.log";
            let _dockerd = start(
                "dockerd",
                &[
                    "--containerd",
                    CONTAINERD,
                    "--data-root",
                    "/var/lib/docker",
                    "--storage-driver",
                    "overlay2",
                    "--iptables=false",
                    "--ip6tables=false",
                    "--bridge=none",
                ],
                dockerd_log,
                "/run/docker.sock",
            );
            let logs = [dockerd_log, containerd_log];
            let loaded = run("docker", &["load", "--input", ARCHIVE], &logs);
            // "Loaded image: NAME", or "Loaded image ID: ID".
            let image = loaded.lines().filter_map(|line| line.split(": ").nth(1)).last().unwrap();
            let said = run(
                "docker",
                &["run", "--rm", "--network", "none", image, "/bin/busybox", "echo", HELLO],
                &logs,
            );
            assert_eq!(said, format!("{HELLO}\n"), "Docker's container");
            let driver = run("docker", &["info", "--format", "{{.Driver}}"], &logs);
            assert_eq!(driver, "overlay2\n");
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
