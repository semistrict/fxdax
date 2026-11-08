// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! How a guest asks the sandbox manager for shared volumes (backlog doc-10), over a stream, one
//! ask after another: the automounter, to attach a base (TASK-136.6), and `git-remote-fxfs`, for
//! the newest generation of a repo it fetches (TASK-136.7):
//!
//! ```text
//! attach NAME\n                 the guest asks for the base named NAME
//! manifest SIZE\n<SIZE bytes>   the host attached it: the manifest of its volume
//! refused WHY\n                 the host attaches nothing of that name for this sandbox
//!
//! newest URL\n                  the guest asks for the newest generation of the repo at URL
//! named NAME\n                  the base it is, which an attach of NAME attaches
//! refused WHY\n                 the host has none for this sandbox
//! ```
//!
//! NAME is a [`crate::valid_name`]; SIZE at most [`MAX_MANIFEST`]; URL and WHY lines, each at
//! most 4 KiB with its newline.

use std::io::{self, BufRead, BufReader, Read, Write};

/// The largest manifest a guest takes: fs/fxfs's `FXFS_MANIFEST_MAX`.
pub const MAX_MANIFEST: usize = 65536;

/// What the sandbox manager answers an attach with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// It attached the base: the manifest of its volume, to import.
    Manifest(Vec<u8>),
    /// It attaches nothing of that name, for why.
    Refused(String),
}

/// What the sandbox manager answers an ask for a repo's newest generation with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Newest {
    /// The name of the base it is.
    Named(String),
    /// It has none for this sandbox, for why.
    Refused(String),
}

/// What answers a guest's asks: the sandbox manager.
pub trait Manager {
    /// Attaches the base `name`, a [`crate::valid_name`], to the sandbox's store.
    fn attach(&mut self, name: &str) -> Answer;
    /// The newest generation of the repo at `url`.
    fn newest(&mut self, url: &str) -> Newest;
}

fn invalid(what: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

/// Asks, on `stream`, for the base `name`, and returns the answer.
pub fn ask(stream: &mut (impl Read + Write), name: &str) -> io::Result<Answer> {
    if !crate::valid_name(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("no volume is {name:?}")));
    }
    let (line, mut reader) = exchange(stream, &format!("attach {name}"))?;
    match line.split_once(' ') {
        Some(("manifest", size)) => {
            let size: usize = size.parse().map_err(|_| invalid(format!("not a size: {line}")))?;
            if size > MAX_MANIFEST {
                return Err(invalid(format!("a manifest of {size} bytes")));
            }
            let mut manifest = vec![0; size];
            reader.read_exact(&mut manifest)?;
            Ok(Answer::Manifest(manifest))
        }
        Some(("refused", why)) => Ok(Answer::Refused(why.to_owned())),
        _ => Err(invalid(format!("not an answer: {line:?}"))),
    }
}

/// Asks, on `stream`, for the newest generation of the repo at `url`, and returns the answer.
pub fn ask_newest(stream: &mut (impl Read + Write), url: &str) -> io::Result<Newest> {
    if url.is_empty() || url.contains('\n') || url.len() >= 4096 - "newest \n".len() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("no URL: {url:?}")));
    }
    let (line, _) = exchange(stream, &format!("newest {url}"))?;
    match line.split_once(' ') {
        Some(("named", name)) if crate::valid_name(name) => Ok(Newest::Named(name.to_owned())),
        Some(("refused", why)) => Ok(Newest::Refused(why.to_owned())),
        _ => Err(invalid(format!("not an answer: {line:?}"))),
    }
}

/// Sends `ask`'s line on `stream`, and returns the answer's first line, and what reads the rest.
fn exchange<'a, S: Read + Write>(
    stream: &'a mut S,
    ask: &str,
) -> io::Result<(String, BufReader<&'a mut S>)> {
    stream.write_all(format!("{ask}\n").as_bytes())?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let line = read_line(&mut reader)?.ok_or_else(|| invalid("the host hung up".into()))?;
    Ok((line, reader))
}

/// Answers the asks that come on `stream`, with what `manager` says, until it ends. A name that
/// is no volume's is refused unasked, and so is a generation named so.
pub fn answer(stream: impl Read + Write, manager: &mut impl Manager) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    while let Some(line) = read_line(&mut reader)? {
        let said = match line.split_once(' ') {
            Some(("attach", name)) if crate::valid_name(name) => match manager.attach(name) {
                Answer::Manifest(manifest) => {
                    let mut said = format!("manifest {}\n", manifest.len()).into_bytes();
                    said.extend_from_slice(&manifest);
                    said
                }
                Answer::Refused(why) => refused(&why),
            },
            Some(("attach", name)) => refused(&format!("no volume is {name:?}")),
            Some(("newest", url)) => match manager.newest(url) {
                Newest::Named(name) if crate::valid_name(&name) => {
                    format!("named {name}\n").into_bytes()
                }
                Newest::Named(name) => refused(&format!("no volume is {name:?}")),
                Newest::Refused(why) => refused(&why),
            },
            _ => return Err(invalid(format!("not an ask: {line:?}"))),
        };
        let stream = reader.get_mut();
        stream.write_all(&said)?;
        stream.flush()?;
    }
    Ok(())
}

fn refused(why: &str) -> Vec<u8> {
    format!("refused {}\n", why.replace('\n', " ")).into_bytes()
}

/// A line of `reader`, without its newline, or none at its end. At most 4 KiB.
fn read_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    reader.by_ref().take(4096).read_until(b'\n', &mut line)?;
    if line.is_empty() {
        return Ok(None);
    }
    if line.pop() != Some(b'\n') {
        return Err(invalid("a line too long, or cut short".into()));
    }
    String::from_utf8(line).map(Some).map_err(|_| invalid("a line not UTF-8".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    /// A manager with a base `linux`, the newest generation of `https://git/linux`, and the asks
    /// it was asked, in order.
    #[derive(Default)]
    struct Host {
        asked: Vec<String>,
    }

    impl Manager for Host {
        fn attach(&mut self, name: &str) -> Answer {
            self.asked.push(format!("attach {name}"));
            match name {
                "linux" => Answer::Manifest(vec![7; 600]),
                _ => Answer::Refused(format!("{name} is\nnot yours")),
            }
        }
        fn newest(&mut self, url: &str) -> Newest {
            self.asked.push(format!("newest {url}"));
            match url {
                "https://git/linux" => Newest::Named("linux".into()),
                "https://git/bad" => Newest::Named("../bad".into()),
                _ => Newest::Refused(format!("nothing of {url}")),
            }
        }
    }

    fn answering(host: UnixStream) -> std::thread::JoinHandle<Vec<String>> {
        std::thread::spawn(move || {
            let mut manager = Host::default();
            answer(host, &mut manager).unwrap();
            manager.asked
        })
    }

    /// A guest's asks of a host answering them: a manifest, a refusal, a name no volume has
    /// refused without asking, and the host's asks in order.
    #[test]
    fn asks_and_answers() {
        let (mut guest, host) = UnixStream::pair().unwrap();
        let answering = answering(host);
        assert_eq!(ask(&mut guest, "linux").unwrap(), Answer::Manifest(vec![7; 600]));
        assert_eq!(ask(&mut guest, "other").unwrap(), Answer::Refused("other is not yours".into()));
        assert_eq!(ask(&mut guest, "linux").unwrap(), Answer::Manifest(vec![7; 600]));
        let error = ask(&mut guest, "../escape").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        drop(guest);
        assert_eq!(answering.join().unwrap(), ["attach linux", "attach other", "attach linux"]);
    }

    /// A repo's newest generation, by its URL, as a name; refused if the manager has none, or
    /// names what can't be a volume.
    #[test]
    fn newest_generations() {
        let (mut guest, host) = UnixStream::pair().unwrap();
        let answering = answering(host);
        let newest = |guest: &mut UnixStream, url| ask_newest(guest, url).unwrap();
        assert_eq!(newest(&mut guest, "https://git/linux"), Newest::Named("linux".into()));
        assert_eq!(
            newest(&mut guest, "/mnt/elsewhere"),
            Newest::Refused("nothing of /mnt/elsewhere".into())
        );
        let bad = Newest::Refused("no volume is \"../bad\"".into());
        assert_eq!(newest(&mut guest, "https://git/bad"), bad);
        for url in ["", "a\nb", &"x".repeat(4096)] {
            let error = ask_newest(&mut guest, url).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{url:?}");
        }
        drop(guest);
        let asked = answering.join().unwrap();
        assert_eq!(
            asked,
            ["newest https://git/linux", "newest /mnt/elsewhere", "newest https://git/bad"]
        );
    }

    /// A host's answer out of the protocol is an error, not a manifest: one too large, one cut
    /// short, one that isn't an answer, and none; and a generation that names no volume.
    #[test]
    fn answers_out_of_the_protocol() {
        for (said, expected) in [
            (format!("manifest {}\n", MAX_MANIFEST + 1), io::ErrorKind::InvalidData),
            ("manifest 10\nshort".to_owned(), io::ErrorKind::UnexpectedEof),
            ("yes\n".to_owned(), io::ErrorKind::InvalidData),
            (String::new(), io::ErrorKind::InvalidData),
        ] {
            let (mut guest, mut host) = UnixStream::pair().unwrap();
            host.write_all(said.as_bytes()).unwrap();
            // What it said, and no more: the guest's ask still goes through.
            host.shutdown(std::net::Shutdown::Write).unwrap();
            let error = ask(&mut guest, "linux").unwrap_err();
            assert_eq!(error.kind(), expected, "{said:?}: {error}");
        }
        let (mut guest, mut host) = UnixStream::pair().unwrap();
        host.write_all(b"named ../escape\n").unwrap();
        host.shutdown(std::net::Shutdown::Write).unwrap();
        let error = ask_newest(&mut guest, "https://git/linux").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
    }

    /// A guest's ask for a name no volume has is refused by the host without asking it.
    #[test]
    fn names_of_no_volume_are_refused_unasked() {
        let (mut guest, host) = UnixStream::pair().unwrap();
        let answering = answering(host);
        guest.write_all(b"attach ../escape\n").unwrap();
        let mut reply = String::new();
        BufReader::new(&mut guest).read_line(&mut reply).unwrap();
        assert_eq!(reply, "refused no volume is \"../escape\"\n");
        drop(guest);
        assert!(answering.join().unwrap().is_empty());
    }
}
