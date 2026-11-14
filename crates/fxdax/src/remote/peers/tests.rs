// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Hosts sharing a bucket in this process, each reaching the others by a link that calls their
//! serving fetchers directly: what asking peers saves, and what it does when they fail. The daemon's
//! gRPC links are `extentd::peers`'.

use super::*;
use extent_remote::bucket::{Bucket as _, PutMode};
use extent_remote::cache::Cache;
use extent_remote::fetch::{Budget, Fetcher, Limits, Memory, Priority};
use extent_remote::layout::{self, Contents, Location, test_branch};
use extent_remote::peer::Peers as _;
use extent_remote::store::{Request, Store as Bucket};
use std::sync::atomic::AtomicBool;
use storage_device::ExtentId;

const PARTS: u64 = 8;
const PART: u64 = layout::PART;
const EXTENT: ExtentId = ExtentId::new(1, 1);

/// Bytes that don't compress, so that each part is stored as it is.
pub(crate) fn noise(len: u64) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

/// A bucket holding a segment of `data`, all of which is [`EXTENT`]: where it is.
pub(crate) fn holding(data: &[u8]) -> (Arc<Bucket>, Location) {
    let bucket = Arc::new(Bucket::new(Arc::new(object_store::memory::InMemory::new()), ""));
    let key = layout::segment(test_branch(1), 1);
    let contents =
        Contents { extents: vec![(EXTENT, 0, data.len() as u64)], ..Contents::default() };
    let footer = layout::Chain::new(test_branch(1), 1, 0, None, 0).close(contents);
    let object = layout::Segment { data: data.to_vec(), footer }.encode();
    futures::executor::block_on(bucket.put(&key, object.into(), PutMode::Create)).unwrap();
    (bucket, Location { key, offset: 0, len: data.len() as u64 })
}

/// A host of this process: the fetcher its stores read by, asking its peers, and the one it serves
/// them by, asking none, which share its memory, budget and cache, as a daemon's do.
struct Host {
    reads: Fetcher<Bucket>,
    peers: Arc<Peers>,
    _cache: tempfile::TempDir,
}

/// A link to a host of this process, by its serving fetcher: failing every request, if told to;
/// or asking for each part as stored otherwise than asked, which it has not.
struct InProcess {
    serving: Arc<Fetcher<Bucket>>,
    calls: AtomicU64,
    failing: AtomicBool,
    mismatching: AtomicBool,
}

impl Link for InProcess {
    fn get(&self, asked: Asked) -> BoxFuture<'static, Result<Option<Vec<u8>>, String>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.failing.load(Ordering::Relaxed) {
            return futures::future::ready(Err("unreachable".to_owned())).boxed();
        }
        let (serving, mismatching) =
            (self.serving.clone(), self.mismatching.load(Ordering::Relaxed));
        async move {
            let Asked { key, part, mut stored } = asked;
            stored.crc ^= u32::from(mismatching);
            let served = serving.stored_part(&key, part, &stored).await;
            served.map(|bytes| bytes.map(|bytes| bytes.to_vec())).map_err(|err| err.to_string())
        }
        .boxed()
    }
}

/// Hosts at `addresses`, sharing `bucket`, each with a cache of its own, each asking the others
/// if `peered`; and the links to each, by address.
fn hosts(
    bucket: &Arc<Bucket>,
    addresses: &[&str],
    peered: bool,
) -> (Vec<Host>, HashMap<String, Arc<InProcess>>) {
    let made: Vec<_> = addresses
        .iter()
        .map(|_| {
            let cache = tempfile::tempdir().unwrap();
            let kept = Arc::new(Cache::open(cache.path(), u64::MAX, 0).unwrap());
            let (budget, memory) = (Budget::new(Limits::UNBOUNDED), Memory::new());
            let fetcher =
                || Fetcher::new(bucket.clone(), budget.clone(), memory.clone(), Some(kept.clone()));
            (fetcher(), Arc::new(fetcher()), cache)
        })
        .collect();
    let links: HashMap<String, Arc<InProcess>> = (addresses.iter().zip(&made))
        .map(|(address, (_, serving, _))| {
            let link = InProcess {
                serving: serving.clone(),
                calls: AtomicU64::new(0),
                failing: AtomicBool::new(false),
                mismatching: AtomicBool::new(false),
            };
            ((*address).to_owned(), Arc::new(link))
        })
        .collect();
    let hosts = (addresses.iter().zip(made))
        .map(|(address, (reads, _, cache))| {
            let links = links.clone();
            let peers = Peers::new(*address, move |to| links[to].clone() as Arc<dyn Link>);
            if peered {
                peers.set(addresses);
            }
            Host { reads: reads.asking(peers.clone()), peers, _cache: cache }
        })
        .collect();
    (hosts, links)
}

/// Reads all of [`EXTENT`], at `location`, with `host`'s fetcher, a part at a time.
fn read_all(host: &Host, location: &Location) -> Vec<u8> {
    let mut read = Vec::new();
    for part in 0..location.len.div_ceil(PART) {
        let reading = host.reads.read(EXTENT, location, part * PART, PART, Priority::Demand);
        read.extend(futures::executor::block_on(reading).unwrap());
    }
    read
}

/// The GETs the bucket was sent, whole and of ranges.
pub(crate) fn gets(bucket: &Bucket) -> u64 {
    let stats = bucket.stats();
    stats.requests(Request::Get) + stats.requests(Request::GetRange)
}

/// How many of the parts of the object at `key` the host at `address` owns among `addresses`.
fn owned(addresses: &[&str], address: &str, key: &str) -> u64 {
    let owner = |part| owner_of(addresses.iter().copied(), key, part);
    (0..PARTS).filter(|part| owner(*part) == Some(address)).count() as u64
}

/// Three hosts reading one object fetch each of its parts from the bucket once, its owner's GET,
/// not once each: 16 GETs fewer for its 8 parts. Each host GETs the object's tail itself.
#[test]
fn hosts_asking_their_peers_fetch_each_part_once() {
    let data = noise(PARTS * PART);
    let addresses = ["10.0.0.1:7070", "10.0.0.2:7070", "10.0.0.3:7070"];
    let mut got = Vec::new();
    for peered in [false, true] {
        let (bucket, location) = holding(&data);
        let (hosts, _) = hosts(&bucket, &addresses, peered);
        for host in &hosts {
            assert!(read_all(host, &location) == data, "{} read other data", host.peers.me());
        }
        got.push(gets(&bucket));
        let given: u64 = hosts.iter().map(|host| host.peers.counts().given).sum();
        assert_eq!(given, if peered { 2 * PARTS } else { 0 });
    }
    assert_eq!(got[1], got[0] - 2 * PARTS, "GETs without peers, and with: {got:?}");
}

/// A peer that fails is asked nothing for a while: the parts it owns come from the bucket
/// meanwhile, read whole; once its while is over, it is asked again.
#[test]
fn a_peer_that_fails_is_asked_nothing_for_a_while() {
    let data = noise(PARTS * PART);
    let (bucket, location) = holding(&data);
    let addresses = ["10.0.0.1:7070", "10.0.0.2:7070"];
    let (hosts, links) = hosts(&bucket, &addresses, true);
    assert!(owned(&addresses, addresses[1], &location.key) > 1, "the test needs two parts owned");
    let other = &links[addresses[1]];
    other.failing.store(true, Ordering::Relaxed);
    assert!(read_all(&hosts[0], &location) == data);
    assert_eq!(other.calls.load(Ordering::Relaxed), 1, "asked again while down");
    let counts = hosts[0].peers.counts();
    assert_eq!(counts, Counts { asked: 1, given: 0, missing: 0, failed: 1 });
    assert_eq!(gets(&bucket), 1 + PARTS, "its tail, and each part, from the bucket");

    let member = hosts[0].peers.others.read().unwrap()[0].clone();
    assert!(!member.health.up_at(Instant::now()));
    assert!(member.health.up_at(Instant::now() + DOWN));
}

/// A peer without the part as asked, as one whose tail is of another object at its key, is no
/// failure: the part comes from the bucket, and the peer is asked for the next.
#[test]
fn a_peer_without_the_part_stays_up() {
    let data = noise(PARTS * PART);
    let (bucket, location) = holding(&data);
    let addresses = ["10.0.0.1:7070", "10.0.0.2:7070"];
    let (hosts, links) = hosts(&bucket, &addresses, true);
    let owned = owned(&addresses, addresses[1], &location.key);
    assert!(owned > 1, "the test needs two parts owned");
    let other = &links[addresses[1]];
    other.mismatching.store(true, Ordering::Relaxed);
    assert!(read_all(&hosts[0], &location) == data);
    assert_eq!(other.calls.load(Ordering::Relaxed), owned);
    let counts = hosts[0].peers.counts();
    assert_eq!(counts, Counts { asked: owned, given: 0, missing: owned, failed: 0 });
}

/// Every part has one owner among the hosts, whatever their order; a host added takes about its
/// share, from the others, and no part moves among those that stay.
#[test]
fn ownership_moves_only_with_the_hosts_added() {
    let key = layout::segment(test_branch(1), 1);
    let three = ["10.0.0.1:7070", "10.0.0.2:7070", "10.0.0.3:7070"];
    let four = ["10.0.0.4:7070", "10.0.0.3:7070", "10.0.0.2:7070", "10.0.0.1:7070"];
    let mut moved = 0;
    for part in 0..10_000 {
        let before = owner_of(three, &key, part).unwrap();
        let reversed = owner_of(three.iter().rev().copied(), &key, part).unwrap();
        assert_eq!(before, reversed, "part {part}: two owners by order");
        let after = owner_of(four, &key, part).unwrap();
        if after != before {
            assert_eq!(after, four[0], "part {part} moved between hosts that stayed");
            moved += 1;
        }
    }
    assert!((2_000..3_000).contains(&moved), "the host added took {moved} of 10000");
}

/// A host's list kept, set again, keeps its peers as they were, down or not; one dropped and added
/// again is new.
#[test]
fn peers_set_again_stay_as_they_were() {
    let addresses = ["10.0.0.1:7070", "10.0.0.2:7070"];
    let link: Arc<dyn Link> = Arc::new(InProcess {
        serving: Arc::new(Fetcher::new(
            holding(&[]).0,
            Budget::new(Limits::UNBOUNDED),
            Memory::new(),
            None,
        )),
        calls: AtomicU64::new(0),
        failing: AtomicBool::new(false),
        mismatching: AtomicBool::new(false),
    });
    let peers = Peers::new(addresses[0], move |_| link.clone());
    peers.set(addresses);
    let member = || peers.others.read().unwrap()[0].clone();
    member().health.down("a test");
    peers.set(addresses.iter().rev());
    assert!(!member().health.up_at(Instant::now()), "up again, set again");
    assert_eq!(peers.addresses(), addresses);
    peers.set([addresses[0]]);
    assert_eq!(peers.addresses(), [addresses[0]]);
    peers.set(addresses);
    assert!(member().health.up_at(Instant::now()), "down, added again");
}

/// Only parts of framed objects are asked of peers: never a branch's record, a lease, or a cell's
/// object, which change in place. And a host asks no peer for what it owns.
#[test]
fn hosts_ask_peers_only_for_framed_objects_they_dont_own() {
    let me = "10.0.0.1:7070";
    let peers = Peers::new(me, |_: &str| -> Arc<dyn Link> { panic!("no link made") });
    let segment = layout::segment(test_branch(1), 1);
    assert!(peers.owner(&segment, 0).is_none(), "asked with no peers");
    peers.set([me]);
    assert!(peers.owner(&segment, 0).is_none(), "asked itself");
    assert_eq!(peers.addresses(), [me]);
    for key in [layout::record(test_branch(1)), layout::lease(test_branch(1))] {
        assert!(!extent_remote::peer::served(&key), "{key} served");
    }
    assert!(extent_remote::peer::served(&segment));
}
