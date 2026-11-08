// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `git-remote-fxfs`, Git's remote helper for repos kept as shared volumes (backlog doc-10,
//! Moving to a newer generation; TASK-136.7). Installed through `url.<base>.insteadOf`, as
//!
//! ```text
//! git config --global url.fxfs::https://github.com/.insteadOf https://github.com/
//! ```
//!
//! it runs first on every fetch of those URLs, which Git has no hook for. It asks the sandbox
//! manager for the newest generation of the repo at the URL, looks up its path under the
//! automounter's root, which attaches and mounts it, and points the repo's alternates at its
//! objects, in place of an earlier generation's: the fetch then transfers nothing that generation
//! holds, as Git sends what its alternates' refs hold as haves. Each generation holds every object
//! of the earlier ones, so none is lost. A refusal, or anything failing, leaves the repo as it is,
//! and the fetch ordinary. Then it hands the URL to Git's own transport: `git remote-https` and
//! the like for a URL of theirs, and, for a local path, Git's `connect` to `git upload-pack`.

use crate::upcall::Newest;
use anyhow::{Context as _, bail};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

/// What tells the helper a repo's newest generation: the sandbox manager.
pub trait Ask {
    fn newest(&mut self, url: &str) -> io::Result<Newest>;
}

/// Points the alternates of the repo at `git_dir` at the objects of the newest generation of the
/// repo at `url`, under `root`, if `ask` names one: whether it did. Why it didn't goes to the
/// log.
pub fn move_to_newest(git_dir: &Path, url: &str, root: &Path, ask: &mut impl Ask) -> bool {
    match try_move(git_dir, url, root, ask) {
        Ok(moved) => moved,
        Err(error) => {
            eprintln!("git-remote-fxfs: fetching without a newer generation: {error:#}");
            false
        }
    }
}

fn try_move(git_dir: &Path, url: &str, root: &Path, ask: &mut impl Ask) -> anyhow::Result<bool> {
    let name = match ask.newest(url).context("asking the sandbox manager")? {
        Newest::Named(name) => name,
        Newest::Refused(why) => {
            eprintln!("git-remote-fxfs: no generation of {url}: {why}");
            return Ok(false);
        }
    };
    // Looked up, it mounts.
    let objects = root.join(&name).join(".git/objects");
    if !objects.is_dir() {
        bail!("generation {name} holds no objects at {}", objects.display());
    }
    let alternates = git_dir.join("objects/info/alternates");
    let before = match std::fs::read_to_string(&alternates) {
        Ok(before) => before,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).context("reading the repo's alternates"),
    };
    let after = alternates_moved(&before, root, &objects);
    if after != before {
        std::fs::create_dir_all(alternates.parent().unwrap())?;
        let new = alternates.with_extension("fxfs-new");
        std::fs::write(&new, &after)?;
        std::fs::rename(&new, &alternates)?;
    }
    Ok(true)
}

/// The alternates `before`, a file's lines, with `objects` in place of any under `root`, an
/// earlier generation's, and the others kept, in order.
pub fn alternates_moved(before: &str, root: &Path, objects: &Path) -> String {
    let mut after: Vec<&str> =
        before.lines().filter(|line| !Path::new(line).starts_with(root)).collect();
    let objects = objects.to_str().expect("a generation's path is UTF-8");
    after.push(objects);
    after.iter().map(|line| format!("{line}\n")).collect()
}

/// How Git's own transport takes a URL.
#[derive(Debug, PartialEq, Eq)]
pub enum Transport {
    /// Its remote helper, `git remote-SCHEME`.
    Helper(String),
    /// A repo on this machine, at the path, served by `git upload-pack` and `receive-pack`
    /// through `connect`.
    Local(PathBuf),
}

/// How Git's own transport takes `url`: with a remote helper for those of Git's own, and on this
/// machine for paths and `file://`. Others, such as ssh's, which Git serves itself, not through a
/// helper, are refused.
pub fn transport(url: &str) -> anyhow::Result<Transport> {
    if let Some(path) = url.strip_prefix("file://") {
        return Ok(Transport::Local(path.into()));
    }
    match url.split_once("://") {
        Some((scheme @ ("http" | "https" | "ftp" | "ftps"), _)) => {
            Ok(Transport::Helper(scheme.to_owned()))
        }
        Some((scheme, _)) => bail!("git-remote-fxfs takes no {scheme}:// URL"),
        None if url.starts_with('/') => Ok(Transport::Local(url.into())),
        None => bail!("git-remote-fxfs takes no URL {url:?}"),
    }
}

/// Speaks Git's remote-helper protocol on `input` and `output` for a repo on this machine: it
/// offers `connect` alone, and returns the service Git connects to, `git-upload-pack` or
/// `git-receive-pack`, once it said it is ready for it, as Git's stream from then on is the
/// service's. None if Git asked nothing more.
pub fn connect(
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> anyhow::Result<Option<String>> {
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        match line.trim_end_matches('\n') {
            "capabilities" => output.write_all(b"connect\n\n")?,
            "" => return Ok(None),
            ask => match ask.strip_prefix("connect ") {
                Some(service @ ("git-upload-pack" | "git-receive-pack")) => {
                    output.write_all(b"\n")?;
                    output.flush()?;
                    return Ok(Some(service.to_owned()));
                }
                _ => bail!("git asked {ask:?}"),
            },
        }
        output.flush()?;
    }
}

/// Runs the helper, as Git does, for the remote `remote` at `url`, its repo at `git_dir` if Git
/// says, the automounter's root `root`, asking `ask`: it returns only if it failed.
pub fn run(
    remote: &str,
    url: &str,
    git_dir: Option<&Path>,
    root: &Path,
    ask: &mut impl Ask,
) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    let transport = transport(url)?;
    match git_dir {
        Some(git_dir) => {
            move_to_newest(git_dir, url, root, ask);
        }
        None => eprintln!("git-remote-fxfs: Git named no repo (GIT_DIR): fetching as it is"),
    }
    let error = match transport {
        Transport::Helper(scheme) => std::process::Command::new("git")
            .arg(format!("remote-{scheme}"))
            .args([remote, url])
            .exec(),
        Transport::Local(path) => {
            // Read a byte at a time: what Git sends after `connect` is the service's.
            let mut input = io::BufReader::with_capacity(1, io::stdin().lock());
            let Some(service) = connect(&mut input, &mut io::stdout().lock())? else {
                return Ok(());
            };
            let service = service.strip_prefix("git-").unwrap();
            std::process::Command::new("git").arg(service).arg(path).exec()
        }
    };
    Err(error).context("handing the fetch to Git")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternates_move_to_the_newest_generation() {
        let root = Path::new("/repos");
        let newest = Path::new("/repos/linux-g2/.git/objects");
        assert_eq!(alternates_moved("", root, newest), "/repos/linux-g2/.git/objects\n");
        let before = "/elsewhere/objects\n/repos/linux-g1/.git/objects\n/repo-sitter/objects\n";
        assert_eq!(
            alternates_moved(before, root, newest),
            "/elsewhere/objects\n/repo-sitter/objects\n/repos/linux-g2/.git/objects\n"
        );
        let moved = alternates_moved(before, root, newest);
        assert_eq!(alternates_moved(&moved, root, newest), moved);
    }

    #[test]
    fn transports() {
        let local = |path: &str| Ok(Transport::Local(path.into()));
        let helper = |scheme: &str| Ok(Transport::Helper(scheme.into()));
        for (url, expected) in [
            ("https://github.com/torvalds/linux", helper("https")),
            ("http://git/linux", helper("http")),
            ("/repos/upstream", local("/repos/upstream")),
            ("file:///repos/upstream", local("/repos/upstream")),
        ] {
            assert_eq!(transport(url).map_err(|error| error.to_string()), expected, "{url}");
        }
        for url in ["ssh://host/repo", "host:repo", "relative"] {
            assert!(transport(url).is_err(), "{url}");
        }
    }

    /// Git's side of `connect`: its capabilities asked, the service connected, and nothing read
    /// past the line that asked it, which is the service's.
    #[test]
    fn connects_to_a_service() {
        let mut input =
            io::Cursor::new(b"capabilities\nconnect git-upload-pack\n0014command=ls-refs".to_vec());
        let mut output = Vec::new();
        let service = connect(&mut io::BufReader::with_capacity(1, &mut input), &mut output);
        assert_eq!(service.unwrap().as_deref(), Some("git-upload-pack"));
        assert_eq!(output, b"connect\n\n\n");
        let rest = &input.get_ref()[input.position() as usize..];
        assert_eq!(rest, b"0014command=ls-refs");
        let mut output = Vec::new();
        let asked = connect(&mut io::Cursor::new(b"capabilities\n\n".to_vec()), &mut output);
        assert_eq!(asked.unwrap(), None);
        let refused = connect(&mut io::Cursor::new(b"fetch 1234 HEAD\n".to_vec()), &mut Vec::new());
        assert_eq!(refused.unwrap_err().to_string(), "git asked \"fetch 1234 HEAD\"");
    }

    /// A manager naming generations by URL, one of which has no objects mounted.
    struct Host(&'static str);

    impl Ask for Host {
        fn newest(&mut self, url: &str) -> io::Result<Newest> {
            Ok(match url {
                "/repos/upstream" => Newest::Named(self.0.into()),
                _ => Newest::Refused("not here".into()),
            })
        }
    }

    /// The repo's alternates move to the newest generation's objects, where its root has them;
    /// a refusal, or a generation with nothing there, leaves them as they were.
    #[test]
    fn repos_move_to_the_newest_generation() {
        let dir = tempfile::tempdir().unwrap();
        let (root, git_dir) = (dir.path().join("repos"), dir.path().join("repo/.git"));
        std::fs::create_dir_all(root.join("g1/.git/objects")).unwrap();
        std::fs::create_dir_all(git_dir.join("objects/info")).unwrap();
        let alternates = git_dir.join("objects/info/alternates");

        assert!(!move_to_newest(&git_dir, "/elsewhere", &root, &mut Host("g1")));
        assert!(!alternates.exists());
        assert!(!move_to_newest(&git_dir, "/repos/upstream", &root, &mut Host("g0")));
        assert!(!alternates.exists());
        assert!(move_to_newest(&git_dir, "/repos/upstream", &root, &mut Host("g1")));
        let expected = format!("{}\n", root.join("g1/.git/objects").display());
        assert_eq!(std::fs::read_to_string(&alternates).unwrap(), expected);
    }
}
