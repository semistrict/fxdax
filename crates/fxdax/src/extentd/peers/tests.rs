// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The daemon's peers over gRPC: parts served as asked, refusals, and a peer out of reach. What
//! asking peers saves, and how it falls back, `crate::remote::peers`' tests show, and two daemons
//! `tests/peers.rs`.

use super::*;
use crate::remote::peers::tests::{gets, holding, noise};
use extent_remote::cache::Cache;
use extent_remote::layout::{self, Location};
use extent_remote::store::Store as Bucket;
use std::time::Instant;

const URL: &str = "memory:///peers";

/// A daemon's peer server on the loopback, serving the parts of `bucket`, named [`URL`], kept in a
/// cache in `dir`: its address.
fn serving(bucket: &Arc<Bucket>, dir: &std::path::Path) -> String {
    let cache = Arc::new(Cache::open(dir, u64::MAX, 0).unwrap());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let serving = crate::remote::serving_peers(bucket.clone(), cache);
    let server = Server { bucket: URL.to_owned(), serving };
    std::thread::spawn(move || serve(listener, server));
    address
}

/// Asks the peer at `address`, serving the bucket at `url`, for `asked`.
fn ask(address: &str, url: &str, asked: Asked) -> Result<Option<Vec<u8>>, String> {
    crate::remote::runtime().handle.block_on(link(address, url).get(asked))
}

/// Part `part` of the object at `location`'s key in `bucket`, as its tail says it is stored, and
/// its stored bytes.
fn asked(bucket: &Bucket, location: &Location, part: u64) -> (Asked, Vec<u8>) {
    let object = crate::remote::runtime().handle.block_on(async {
        use extent_remote::bucket::Bucket as _;
        bucket.get(&location.key).await.unwrap().bytes
    });
    let (_, stored) = extent_frame::Parts::of(&object).unwrap().part(part).unwrap();
    let bytes = object[stored.range().start as usize..stored.range().end as usize].to_vec();
    (Asked { key: location.key.clone(), part, stored }, bytes)
}

/// A part is served as stored, fetched from the bucket once, its object's tail and itself, and
/// then from the host's memory.
#[test]
fn peers_are_served_parts_over_grpc() {
    let (bucket, location) = holding(&noise(3 * layout::PART));
    let dir = tempfile::tempdir().unwrap();
    let address = serving(&bucket, dir.path());
    let (asked, bytes) = asked(&bucket, &location, 1);
    let before = gets(&bucket);
    assert_eq!(ask(&address, URL, asked.clone()), Ok(Some(bytes.clone())));
    assert_eq!(gets(&bucket) - before, 2, "its tail and the part");
    assert_eq!(ask(&address, URL, asked), Ok(Some(bytes)));
    assert_eq!(gets(&bucket) - before, 2, "fetched again");
    assert_eq!(SERVED.load(Ordering::Relaxed), 2);
}

/// A part asked for as stored otherwise than the host's tail says, as by a peer whose tail is of
/// another object at the key, isn't there to give: NOT_FOUND, no failure.
#[test]
fn parts_asked_otherwise_are_not_found() {
    let (bucket, location) = holding(&noise(2 * layout::PART));
    let dir = tempfile::tempdir().unwrap();
    let address = serving(&bucket, dir.path());
    let (mut asked, _) = asked(&bucket, &location, 0);
    asked.stored.crc ^= 1;
    assert_eq!(ask(&address, URL, asked), Ok(None));
    assert_eq!(SERVED_MISSING.load(Ordering::Relaxed), 1);
}

/// Peers asking for what is not served, or of another bucket, are refused: failures, which put
/// the peer down for the asker.
#[test]
fn peers_asking_wrongly_are_refused() {
    let (bucket, location) = holding(&noise(layout::PART));
    let dir = tempfile::tempdir().unwrap();
    let address = serving(&bucket, dir.path());
    let (asked, _) = asked(&bucket, &location, 0);
    let refused = |url: &str, asked: Asked| ask(&address, url, asked).unwrap_err();

    let record = layout::record(layout::test_branch(1));
    let not_served = Asked { key: record.clone(), ..asked.clone() };
    assert_eq!(refused(URL, not_served), format!("InvalidArgument: {record}: no parts served"));
    let other = "memory:///other";
    assert_eq!(
        refused(other, asked.clone()),
        format!("FailedPrecondition: asked of {other}, this host's bucket being {URL}")
    );
    let mut empty = asked;
    empty.stored.len = 0;
    let key = &location.key;
    assert_eq!(refused(URL, empty), format!("InvalidArgument: part 0 of {key}, of 0 bytes"));
}

/// A peer out of reach fails within the time to connect, not the request's deadline.
#[test]
fn a_peer_out_of_reach_fails_fast() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);
    let (bucket, location) = holding(&noise(layout::PART));
    let (asked, _) = asked(&bucket, &location, 0);
    let began = Instant::now();
    let failed = ask(&address, URL, asked).unwrap_err();
    assert!(failed.starts_with("Unavailable: "), "{failed}");
    assert!(began.elapsed() < CONNECT * 2, "took {:?}", began.elapsed());
}

/// Peers are named `HOST:PORT`, nothing more.
#[test]
fn peers_are_named_host_and_port() {
    for address in ["10.0.0.1:7070", "host-a.internal:7070", "[::1]:7070"] {
        assert_eq!(check_address(address), Ok(()), "{address}");
    }
    for address in ["10.0.0.1", "http://10.0.0.1:7070", "10.0.0.1:7070/x", "", ":7070"] {
        assert!(check_address(address).is_err(), "{address:?} taken");
    }
}
