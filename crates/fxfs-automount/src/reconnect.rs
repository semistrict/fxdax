// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A guest's exchange with the host over vsock, on a connection of its own, made again while a
//! restore from a snapshot resets it (TASK-277).
//!
//! A VM restored from a snapshot has no connection the snapshotted VM had: its vsock device resets
//! them all, and the guest's kernel takes the resets as the restored VM runs. A connection made
//! as the snapshot was taken, or just after the restore, before the guest took them, may be
//! reset too: its `connect` fails with `ECONNRESET`, or it succeeds, and then a write fails
//! (`EPIPE`, `ENOTCONN`, `ECONNRESET`), or a read ends with nothing of the host's answer read, or
//! part of it. The guest makes such an exchange again, on a new connection, which reaches the
//! restored VM's host. Only an exchange the host may see twice is made again: an ask it answers
//! the same however often it is asked, as the sandbox manager's are (the automounter's `attach`,
//! `git-remote-fxfs`'s `newest`).

use std::io;
use std::time::Duration;

/// How many times an exchange is made at most: the resets are the guest kernel's to take, which
/// it does as soon as the restored VM runs, so that one more is the most a restore needs; more,
/// for one restored again as it takes them.
pub const ATTEMPTS: usize = 100;

/// How long the guest waits before it makes again an exchange a reset ended: a moment for its
/// kernel to take the resets.
pub const PAUSE: Duration = Duration::from_millis(10);

/// Whether `error` ended an exchange as a restore's reset of its connection does: refused while it
/// connected (`ECONNRESET`), cut off as it wrote (`EPIPE`, `ENOTCONN`, `ECONNABORTED`), or ended
/// before the whole answer was read ([`io::ErrorKind::UnexpectedEof`], which an exchange returns
/// for an answer cut short).
pub fn reset_by_restore(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::NotConnected
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof
    )
}

/// Makes `exchange` on a connection `connect` makes, and again on a new one while a reset, as a
/// restore's ([`reset_by_restore`]), ends it, at most [`ATTEMPTS`] times, [`PAUSE`] apart. Any
/// other error is returned at once; the last reset, once the attempts are spent.
pub fn exchange<S, T>(
    mut connect: impl FnMut() -> io::Result<S>,
    mut exchange: impl FnMut(&mut S) -> io::Result<T>,
) -> io::Result<T> {
    let mut attempt = 1;
    loop {
        match connect().and_then(|mut stream| exchange(&mut stream)) {
            Err(error) if reset_by_restore(&error) && attempt < ATTEMPTS => {
                eprintln!("fxfs-automount: the host's connection was reset ({error}): again");
                attempt += 1;
                std::thread::sleep(PAUSE);
            }
            done => return done,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resets_are_made_again() {
        // Each exchange ends as the next of these says, the last first, and then is answered.
        let mut ends = vec![
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::BrokenPipe,
        ];
        let mut made = 0;
        let answer = exchange(
            || Ok(()),
            |()| {
                made += 1;
                match ends.pop() {
                    Some(kind) => Err(io::Error::from(kind)),
                    None => Ok("answered"),
                }
            },
        );
        assert_eq!(answer.unwrap(), "answered");
        assert_eq!(made, 4);
    }

    #[test]
    fn a_connect_reset_is_made_again() {
        let mut connects = 0;
        let answer = exchange(
            || {
                connects += 1;
                match connects {
                    1 => Err(io::Error::from(io::ErrorKind::ConnectionReset)),
                    _ => Ok(()),
                }
            },
            |()| Ok(7),
        );
        assert_eq!(answer.unwrap(), 7);
        assert_eq!(connects, 2);
    }

    #[test]
    fn other_errors_are_returned_at_once() {
        let mut made = 0;
        let answer: io::Result<()> = exchange(
            || Ok(()),
            |()| {
                made += 1;
                Err(io::Error::from(io::ErrorKind::InvalidData))
            },
        );
        assert_eq!(answer.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(made, 1);
    }

    #[test]
    fn resets_stop_once_the_attempts_are_spent() {
        let mut made = 0;
        let answer: io::Result<()> = exchange(
            || Ok(()),
            |()| {
                made += 1;
                Err(io::Error::from(io::ErrorKind::ConnectionReset))
            },
        );
        assert_eq!(answer.unwrap_err().kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(made, ATTEMPTS);
    }
}
