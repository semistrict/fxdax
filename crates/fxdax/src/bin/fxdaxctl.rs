// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's host tool for VMs' disks, the extent stores extentd serves: makes them, of branches kept
//! in object storage (backlog doc-7) or of container images, places, forks, deletes and resizes
//! them, makes them public bases, and waits for their uploads:
//!
//! ```sh
//! fxdaxctl take DIR CAPACITY BRANCH [TOKEN]
//! fxdaxctl place DIR TOKEN
//! fxdaxctl sync DIR
//! fxdaxctl import DIR CAPACITY IMAGE [REF]
//! fxdaxctl fork SRC DST
//! fxdaxctl point SRC DST
//! fxdaxctl delete DIR
//! fxdaxctl drop DIR
//! fxdaxctl resize DIR CAPACITY
//! fxdaxctl public DIR
//! fxdaxctl stat DIR
//! fxdaxctl stats [--json]
//! fxdaxctl --version
//! ```
//!
//! With `FXDAX_EXTENTD_GRPC` naming the control socket of the extent daemon (TASK-43.5), every
//! command goes through its control API (`fxdax::extentd::control`), and acts on stores as the
//! daemon serves them: on a store a VM has open, live, or not at all (`EBUSY`). Without, fxdaxctl
//! acts on stores itself, on stores nothing has open, as their lock says, which an extent daemon
//! serving them holds (it waits a while for it, then refuses). `take` and `import`, which make new
//! stores, work so anywhere, as when building base images where no daemon runs. The others change
//! existing stores' branches, which are in the bucket each store's description names: they need
//! this host's (`FXDAX_EXTENTD_REMOTE`) and its placement's fencing token (`FXDAX_EXTENTD_TOKEN`),
//! and refuse otherwise (`fxdax::remote::Host`, TASK-45).
//!
//! `take` makes an empty store at DIR, of CAPACITY bytes, of BRANCH (hex): once extentd opens it,
//! with `FXDAX_EXTENTD_REMOTE` naming the branch's bucket, it takes what the branch holds there,
//! whose data it fetches as it is read; placed at TOKEN, if given, as `place` places it. A
//! snapshot's point it reads, taking no lease, as any number of hosts do at once. `place`
//! places the store at DIR at the placement whose fencing token,
//! its boot ID, is TOKEN (TASK-39.7): its writer takes its branch in object storage with that token
//! from then on, which a later placement's exceeds, so that a placement cut off takes no branch
//! back from the one after it (`ESTALE`), and its commits fail once a later one takes it. `sync`
//! returns once every commit of the store at DIR is durable and in object storage, as before a
//! clean stop: through the daemon, a store a VM has open as of then. `import` makes a store at DIR, of CAPACITY bytes,
//! holding an Fxfs filesystem of the container image IMAGE (TASK-39.5), a base for VMs to boot
//! forks of: IMAGE is an OCI image layout, as `skopeo copy` makes, or `docker save` (whose tarball
//! is one, once extracted), holding the image REF names, or its only image; or a tar archive of one
//! layer, gzipped or not, as `docker export` makes. Files keep their owners, permissions, times
//! and extended attributes; see `fxdax::import`. `fork` copies the store at SRC to a new store at
//! DST, of a new branch, which it prints: through the daemon, live, if a VM has SRC open, once
//! nothing is open on it, as with the guest's filesystem frozen (fsfreeze), and refused otherwise
//! (TASK-39.3). The new branch is a fork of SRC's in its bucket, made once SRC's commits are
//! uploaded, holding its extents without uploading them again; DST is a store of its own, which
//! reads them there (TASK-165.7). `point` forks so, once nothing is open, to a point (TASK-158): a name of SRC's
//! state as it is, written by none, which any number of hosts fork at once, each from a copy `take`
//! makes of its branch, as a snapshot's restores do; for a base forked on several hosts. `delete`
//! deletes a store, a fork or not: its branch in its bucket, its forks' living on, and its
//! directory (TASK-39.4). `drop` deletes the store at DIR as `delete` does, this
//! host's copy of it, its branch staying in the bucket (TASK-154): what `take` made, once this host
//! is done with it. `resize` sets the store's capacity, the size of the disk
//! its guest sees, to CAPACITY bytes: through the daemon, a store its VM has open grows at once,
//! the guest seeing the new size while mounted, and shrinks if the guest's filesystem agrees, no
//! lower than what it holds; otherwise, it grows, and does not shrink. `public` makes the store at
//! DIR a public base, whose pages every tenant's forks of it share (TASK-43). `stat` prints what the
//! store at DIR is and holds, one `name value` a line: its branch (hex), capacity, used and
//! available bytes, and of those used, its own (its branch wrote them) and inherited (from the
//! branches it was forked from), what billing it alone charges it and what it shares (TASK-49);
//! whether it is public, and, through the daemon, open. `--version` prints the commit it was built
//! of (`fxdax::BUILD_COMMIT`, TASK-111.7).
//!
//! `stats`, through the daemon alone, prints what the daemon, its VM processes and their guests
//! counted, as of one moment, each with the commit it was built of (the control API's
//! `GetSnapshot`, TASK-111.4): with `--json`, as one JSON object, of `taken_unix_ns`, `extentd`
//! (`build`, `counters`, `gauges`) and `vms`, each VM process's `connection`, `branch` (hex),
//! `path`, `follower`, `vmm` (`build`, `counters`, `gauges`), `window`, `answered`, `answer_ns` and
//! `guest` (`build`, `counters`, `histograms`), null where it has none; otherwise as lines of a
//! path through that object and a value, `vms.0.guest.counters.seals 12`. Counters only grow: a
//! measurement is the difference of two.

use anyhow::{Context as _, bail};
use fxdax::ExtentStore;
use std::path::{Path, PathBuf};

fn usage() -> anyhow::Error {
    anyhow::anyhow!(
        "usage: fxdaxctl take DIR CAPACITY BRANCH [TOKEN] | fxdaxctl place DIR TOKEN | \
         fxdaxctl sync DIR | fxdaxctl import DIR CAPACITY IMAGE [REF] | fxdaxctl fork SRC DST | \
         fxdaxctl point SRC DST | fxdaxctl delete DIR | fxdaxctl drop DIR | \
         fxdaxctl resize DIR CAPACITY | fxdaxctl public DIR | \
         fxdaxctl public-generation DIR | fxdaxctl attach DIR BASE | fxdaxctl stat DIR | \
         fxdaxctl stats [--json] | fxdaxctl --version"
    )
}

/// What fxdaxctl was asked to do.
enum Command {
    Take {
        dir: PathBuf,
        capacity: u64,
        branch: u64,
        token: Option<u64>,
    },
    Place {
        dir: PathBuf,
        token: u64,
    },
    Sync {
        dir: PathBuf,
    },
    Import {
        dir: PathBuf,
        capacity: u64,
        image: PathBuf,
        reference: Option<String>,
    },
    /// A fork, a point if `point`.
    Fork {
        src: PathBuf,
        dst: PathBuf,
        point: bool,
    },
    Delete {
        dir: PathBuf,
    },
    Drop {
        dir: PathBuf,
    },
    Resize {
        dir: PathBuf,
        capacity: u64,
    },
    Public {
        dir: PathBuf,
    },
    PublicGeneration {
        dir: PathBuf,
    },
    Attach {
        dir: PathBuf,
        base: PathBuf,
    },
    Stat {
        dir: PathBuf,
    },
    /// fxdax: a snapshot of the daemon's, its VM processes' and their guests' counters, as JSON if
    /// `json` (TASK-111.4).
    Stats {
        json: bool,
    },
}

impl Command {
    fn parse(args: &[String]) -> anyhow::Result<Self> {
        let capacity = |capacity: &str| capacity.parse().context("CAPACITY must be a number");
        let token = |token: &str| token.parse().context("TOKEN must be a number");
        Ok(match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
            ["take", dir, size, branch, ref rest @ ..] if rest.len() <= 1 => Self::Take {
                dir: dir.into(),
                capacity: capacity(size)?,
                branch: u64::from_str_radix(branch, 16).context("BRANCH must be hex")?,
                token: rest.first().map(|rest| token(rest)).transpose()?,
            },
            ["place", dir, given] => Self::Place { dir: dir.into(), token: token(given)? },
            ["sync", dir] => Self::Sync { dir: dir.into() },
            ["import", dir, size, image, ref rest @ ..] if rest.len() <= 1 => Self::Import {
                dir: dir.into(),
                capacity: capacity(size)?,
                image: image.into(),
                reference: rest.first().map(|reference| reference.to_string()),
            },
            ["fork", src, dst] => Self::Fork { src: src.into(), dst: dst.into(), point: false },
            ["point", src, dst] => Self::Fork { src: src.into(), dst: dst.into(), point: true },
            ["delete", dir] => Self::Delete { dir: dir.into() },
            ["drop", dir] => Self::Drop { dir: dir.into() },
            ["resize", dir, size] => Self::Resize { dir: dir.into(), capacity: capacity(size)? },
            ["public", dir] => Self::Public { dir: dir.into() },
            ["public-generation", dir] => Self::PublicGeneration { dir: dir.into() },
            ["attach", dir, base] => Self::Attach { dir: dir.into(), base: base.into() },
            ["stat", dir] => Self::Stat { dir: dir.into() },
            ["stats"] => Self::Stats { json: false },
            ["stats", "--json"] => Self::Stats { json: true },
            _ => bail!(usage()),
        })
    }

    /// Does it through the control API of the daemon whose control socket is at `socket`.
    #[cfg(target_os = "linux")]
    fn through(self, socket: &Path) -> anyhow::Result<()> {
        let daemon = fxdax::extentd::control::Client::connect(socket)?;
        match self {
            Self::Take { dir, capacity, branch, token } => {
                daemon.create_store(&dir, capacity, Some(branch), token)?;
            }
            Self::Place { dir, token } => daemon.place_store(&dir, token)?,
            Self::Sync { dir } => daemon.sync_store(&dir)?,
            Self::Import { dir, capacity, image, reference } => {
                daemon.import_image(&dir, capacity, &image, reference.as_deref())?;
            }
            Self::Fork { src, dst, point: false } => {
                println!("{:x}", daemon.fork_store(&src, &dst)?);
            }
            Self::Fork { src, dst, point: true } => {
                println!("{:x}", daemon.point_store(&src, &dst)?);
            }
            Self::Delete { dir } => daemon.delete_store(&dir)?,
            Self::Drop { dir } => daemon.drop_store(&dir)?,
            Self::Resize { dir, capacity } => daemon.resize_store(&dir, capacity)?,
            Self::Public { dir } => daemon.make_public(&dir)?,
            Self::PublicGeneration { dir } => daemon.make_generation_public(&dir)?,
            Self::Attach { dir, base } => daemon.attach_base(&dir, &base)?,
            Self::Stat { dir } => {
                let stat = daemon.stat_store(&dir)?;
                let lines = stat_lines(
                    stat.branch,
                    stat.capacity,
                    stat.used,
                    stat.available,
                    stat.own,
                    stat.inherited,
                    stat.public,
                    Some(stat.open),
                );
                print!("{lines}");
            }
            Self::Stats { json } => {
                let snapshot = snapshot_json(&daemon.snapshot(None)?);
                match json {
                    true => println!("{}", serde_json::to_string_pretty(&snapshot)?),
                    false => print!("{}", flattened(&snapshot)),
                }
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn through(self, _socket: &Path) -> anyhow::Result<()> {
        bail!("the extent daemon runs on Linux only")
    }

    /// Does it here, on stores nothing has open: new ones anywhere; existing ones with the bucket
    /// and a placement token.
    fn locally(self) -> anyhow::Result<()> {
        if let Self::Stats { .. } = self {
            bail!("stats are the extent daemon's: name its control socket (FXDAX_EXTENTD_GRPC)");
        }
        // What stat reads needs no branch changed, nor does a copy dropped, its branch left be.
        let existing = !matches!(
            self,
            Self::Take { .. } | Self::Import { .. } | Self::Stat { .. } | Self::Drop { .. }
        );
        if let Some(Err(why)) = existing.then(fxdax::remote::Host::from_env) {
            bail!(
                "without the extent daemon (FXDAX_EXTENTD_GRPC), fxdaxctl changes no existing \
                 store's branch here: {why}"
            );
        }
        match self {
            Self::Take { dir, capacity, branch, token } => {
                ExtentStore::create_for_branch(&dir, capacity, branch)?;
                if let Some(token) = token {
                    ExtentStore::place(&dir, token)?;
                }
            }
            Self::Place { dir, token } => ExtentStore::place(&dir, token)?,
            Self::Sync { dir } => {
                let store = ExtentStore::open(&dir)?;
                attach(&store)?;
                store.wait_uploaded()?;
            }
            Self::Import { dir, capacity, image, reference } => {
                let made = fxdax::import::store(&dir, capacity, &image, reference.as_deref());
                futures::executor::block_on(made)
                    .with_context(|| format!("importing {}", image.display()))?;
            }
            Self::Fork { src, dst, point } => {
                let store = ExtentStore::open(&src)?;
                attach(&store)?;
                let branch = rand::random::<u64>().max(1);
                if point {
                    store.point(&dst, branch)?;
                } else {
                    store.fork(&dst, branch)?;
                }
                println!("{branch:x}");
            }
            Self::Delete { dir } => {
                let bucket = ExtentStore::bucket_at(&dir)?;
                ExtentStore::delete(&dir, bucket.as_deref())?
            }
            Self::Drop { dir } => ExtentStore::delete(&dir, None)?,
            Self::Resize { dir, capacity } => {
                let store = ExtentStore::open(&dir)?;
                // Attached, which says whether it is a point, whose capacity changes nowhere
                // (TASK-160.10).
                attach(&store)?;
                if capacity < store.info()?.capacity {
                    bail!(
                        "shrinking a store needs its guest to agree: resize it through the \
                         extent daemon serving its VM (FXDAX_EXTENTD_GRPC)"
                    );
                }
                store.set_capacity(capacity)?;
            }
            Self::Public { dir } => ExtentStore::open(&dir)?.make_public()?,
            Self::PublicGeneration { dir } => ExtentStore::open(&dir)?.make_generation_public()?,
            Self::Attach { dir, base } => {
                let ancestry = ExtentStore::open(&base)?.ancestry();
                ExtentStore::open(&dir)?.attach_base(&base.canonicalize()?, &ancestry)?;
            }
            Self::Stats { .. } => unreachable!("refused above"),
            Self::Stat { dir } => {
                let store = ExtentStore::open(&dir)?;
                let (info, bytes) = (store.info()?, store.bytes()?);
                let lines = stat_lines(
                    store.branch(),
                    info.capacity,
                    info.used,
                    info.available,
                    bytes.own,
                    bytes.inherited,
                    store.is_public(),
                    None,
                );
                print!("{lines}");
            }
        }
        Ok(())
    }
}

/// What `stat` prints, one `name value` a line; whether the store is open, if known.
#[allow(clippy::too_many_arguments)]
fn stat_lines(
    branch: u64,
    capacity: u64,
    used: u64,
    available: u64,
    own: u64,
    inherited: u64,
    public: bool,
    open: Option<bool>,
) -> String {
    let mut lines = format!(
        "branch {branch:x}\ncapacity {capacity}\nused {used}\navailable {available}\nown {own}\n\
         inherited {inherited}\npublic {public}\n"
    );
    if let Some(open) = open {
        lines += &format!("open {open}\n");
    }
    lines
}

/// fxdax: `snapshot` as `stats --json` prints it (TASK-111.4): see the module docs. Maps' keys in
/// order.
#[cfg(target_os = "linux")]
fn snapshot_json(
    snapshot: &fxdax::extentd::control::proto::GetSnapshotResponse,
) -> serde_json::Value {
    use serde_json::{Value, json};
    let named = |map: &std::collections::HashMap<String, u64>| -> Value {
        map.iter().map(|(name, value)| (name.clone(), Value::from(*value))).collect()
    };
    let nonempty = |text: &str| (!text.is_empty()).then(|| text.to_owned());
    let vms = snapshot.vms.iter().map(|vm| {
        let guest = vm.guest.as_ref().map(|guest| {
            let histograms: serde_json::Map<String, Value> = (guest.histograms.iter())
                .map(|(name, histogram)| (name.clone(), json!(histogram.buckets)))
                .collect();
            json!({
                "build": nonempty(&guest.build),
                "counters": named(&guest.counters),
                "histograms": histograms,
            })
        });
        json!({
            "connection": vm.connection,
            "branch": format!("{:x}", vm.branch),
            "path": vm.path,
            "follower": vm.follower,
            "vmm": {
                "build": nonempty(&vm.build),
                "counters": named(&vm.counters),
                "gauges": named(&vm.gauges),
            },
            "window": (!vm.window.is_empty()).then(|| named(&vm.window)),
            "answered": vm.answered,
            "answer_ns": vm.answered.then_some(vm.answer_ns),
            "guest": guest,
        })
    });
    json!({
        "taken_unix_ns": snapshot.taken_unix_ns,
        "extentd": {
            "build": snapshot.build,
            "counters": named(&snapshot.counters),
            "gauges": named(&snapshot.gauges),
        },
        "vms": vms.collect::<Vec<_>>(),
    })
}

/// fxdax: `value`, as lines of a path through it, its keys and indices joined by dots, and a value
/// there that holds no other: what `stats` prints without `--json`.
#[cfg(target_os = "linux")]
fn flattened(value: &serde_json::Value) -> String {
    use serde_json::Value;
    fn lines(at: &str, value: &Value, out: &mut String) {
        let within =
            |name: &str| if at.is_empty() { name.to_owned() } else { format!("{at}.{name}") };
        match value {
            Value::Object(map) => {
                map.iter().for_each(|(name, value)| lines(&within(name), value, out))
            }
            Value::Array(values) => (values.iter().enumerate())
                .for_each(|(index, value)| lines(&within(&index.to_string()), value, out)),
            Value::String(text) => *out += &format!("{at} {text}\n"),
            value => *out += &format!("{at} {value}\n"),
        }
    }
    let mut out = String::new();
    lines("", value, &mut out);
    out
}

fn main() -> anyhow::Result<()> {
    // fxdax: its threads' stacks on SIGQUIT, as an import that hung needed (TASK-98, TASK-44.13).
    #[cfg(target_os = "linux")]
    fxdax::stacks::on_sigquit()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    // fxdax: the commit it was built of (TASK-111.7).
    if args == ["--version"] {
        println!("fxdaxctl {}", fxdax::BUILD_COMMIT);
        return Ok(());
    }
    let command = Command::parse(&args)?;
    match std::env::var_os("FXDAX_EXTENTD_GRPC").filter(|socket| !socket.is_empty()) {
        Some(socket) => command.through(socket.as_ref()),
        None => command.locally(),
    }
}

/// Keeps `store`'s branch in the bucket its description names, as this host's placement.
fn attach(store: &ExtentStore) -> anyhow::Result<()> {
    let token = fxdax::remote::host_token()?;
    let options = fxdax::remote::Options { token, ..Default::default() };
    store.attach_remote(store.bucket().clone(), rand::random(), options)?;
    Ok(())
}
