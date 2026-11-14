// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the hosts sharing this host's bucket, which its fetchers ask for the parts they own
//! before the bucket (TASK-406.2, [`extent_remote::peer`]), so that a part many of them read comes
//! from the bucket about once for them all.
//!
//! Each part of an object has one owner among them, by rendezvous hashing of its key and number
//! with each host's address ([`owner_of`]): every host given the same list finds the same owner,
//! and a host added or removed moves only the parts it gains or had. A host asks the owner, unless
//! it is the owner itself, and the owner gives it from its memory or its cache, or fetches it from
//! the bucket once, however many ask. A peer that fails, as any request of it may, unanswered past
//! its deadline, refused, or a part it gives failing its CRC, is asked nothing for [`DOWN`]: its
//! parts come from the bucket meanwhile, not from another peer, so that no part has two owners.
//!
//! The list is this host's address and the others', each as `HOST:PORT`, spelled as every host
//! spells it: by the environment at the daemon's start, and replaced at any time by the control
//! API, as hosts come and go. How a peer is reached is a [`Link`]'s: gRPC over TCP, in the daemon
//! (`extentd::peers`).

use extent_frame::Stored;
use extent_remote::peer::{self, Peer};
use futures::FutureExt as _;
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::hash::Hasher as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// How long a peer that failed is asked nothing.
pub const DOWN: Duration = Duration::from_secs(30);

/// A part a host asks a peer for: the object's key, the part's number, and where and how the
/// asker's tail says it is stored, which the peer gives only if its own says so too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    /// The object's key.
    pub key: String,
    /// The part's number.
    pub part: u64,
    /// Where and how the asker's tail says it is stored.
    pub stored: Stored,
}

/// How a peer is reached.
pub trait Link: Send + Sync {
    /// The stored bytes of the part `asked` names, from the peer: none if it has no such part, as
    /// stored so; an error if it failed to say, or refused.
    fn get(&self, asked: Asked) -> BoxFuture<'static, Result<Option<Vec<u8>>, String>>;
}

/// What a host's asking of its peers did, since its start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Parts asked of their owners.
    pub asked: u64,
    /// Parts their owners gave, checked: each a GET of the bucket's saved.
    pub given: u64,
    /// Parts their owners didn't have, as asked.
    pub missing: u64,
    /// Peers' failures, each putting it down for [`DOWN`].
    pub failed: u64,
}

#[derive(Default)]
struct Counters {
    asked: AtomicU64,
    given: AtomicU64,
    missing: AtomicU64,
    failed: AtomicU64,
}

/// What reaches the host at an address.
type Connect = Box<dyn Fn(&str) -> Arc<dyn Link> + Send + Sync>;

/// The hosts sharing this host's bucket: this one, by its address, and the others it asks.
pub struct Peers {
    me: String,
    connect: Connect,
    others: RwLock<Vec<Arc<Member>>>,
    counters: Arc<Counters>,
}

/// Another host: how it is reached, and how it fares.
struct Member {
    link: Arc<dyn Link>,
    health: Arc<Health>,
}

/// How a host fares: when it may be asked again, if it failed; and what asking it did, counted
/// with the others'. Its requests in flight hold it, as the list may drop the host meanwhile.
struct Health {
    address: String,
    down_until: Mutex<Option<Instant>>,
    counters: Arc<Counters>,
}

impl Peers {
    /// The peers of the host at `me`, none yet, each reached by the link `connect` gives for its
    /// address.
    pub fn new(
        me: impl Into<String>,
        connect: impl Fn(&str) -> Arc<dyn Link> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            me: me.into(),
            connect: Box::new(connect),
            others: RwLock::default(),
            counters: Arc::default(),
        })
    }

    /// This host's address.
    pub fn me(&self) -> &str {
        &self.me
    }

    /// Has `addresses` be the hosts sharing the bucket, this one's among them or not: those it had
    /// already kept as they were, down or not, their links too; the others reached anew.
    pub fn set(&self, addresses: impl IntoIterator<Item = impl AsRef<str>>) {
        let mut others = self.others.write().unwrap();
        let mut had: HashMap<String, Arc<Member>> =
            others.drain(..).map(|member| (member.health.address.clone(), member)).collect();
        let mut addresses: Vec<String> =
            addresses.into_iter().map(|address| address.as_ref().to_owned()).collect();
        addresses.sort();
        addresses.dedup();
        for address in addresses.into_iter().filter(|address| *address != self.me) {
            let member = had.remove(&address).unwrap_or_else(|| {
                let link = (self.connect)(&address);
                let down_until = Mutex::new(None);
                let health = Health { address, down_until, counters: self.counters.clone() };
                Arc::new(Member { link, health: Arc::new(health) })
            });
            others.push(member);
        }
    }

    /// The hosts sharing the bucket, this one's address first.
    pub fn addresses(&self) -> Vec<String> {
        let others = self.others.read().unwrap();
        std::iter::once(self.me.clone())
            .chain(others.iter().map(|member| member.health.address.clone()))
            .collect()
    }

    /// What its asking did, since its start.
    pub fn counts(&self) -> Counts {
        let count = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        let counters = &self.counters;
        Counts {
            asked: count(&counters.asked),
            given: count(&counters.given),
            missing: count(&counters.missing),
            failed: count(&counters.failed),
        }
    }
}

impl peer::Peers for Peers {
    fn owner(&self, key: &str, part: u64) -> Option<Arc<dyn Peer>> {
        if !peer::served(key) {
            return None;
        }
        let others = self.others.read().unwrap();
        let addresses = std::iter::once(self.me.as_str())
            .chain(others.iter().map(|member| member.health.address.as_str()));
        let owner = owner_of(addresses, key, part)?;
        let member = others.iter().find(|member| member.health.address == owner)?;
        member.health.up_at(Instant::now()).then(|| member.clone() as Arc<dyn Peer>)
    }
}

impl Health {
    /// Whether it may be asked at `now`.
    fn up_at(&self, now: Instant) -> bool {
        self.down_until.lock().unwrap().is_none_or(|until| now >= until)
    }

    /// It failed, as `why` says: asked nothing for [`DOWN`].
    fn down(&self, why: &str) {
        log::warn!("extentd: peer {}: {why}: asked nothing for {DOWN:?}", self.address);
        self.counters.failed.fetch_add(1, Ordering::Relaxed);
        *self.down_until.lock().unwrap() = Some(Instant::now() + DOWN);
    }
}

impl Peer for Member {
    fn part(&self, key: &str, part: u64, stored: Stored) -> BoxFuture<'static, Option<Vec<u8>>> {
        let health = self.health.clone();
        health.counters.asked.fetch_add(1, Ordering::Relaxed);
        let getting = self.link.get(Asked { key: key.to_owned(), part, stored });
        async move {
            match getting.await {
                Ok(Some(bytes)) => {
                    health.counters.given.fetch_add(1, Ordering::Relaxed);
                    Some(bytes)
                }
                Ok(None) => {
                    health.counters.missing.fetch_add(1, Ordering::Relaxed);
                    None
                }
                Err(err) => {
                    health.down(&err);
                    None
                }
            }
        }
        .boxed()
    }

    fn failed(&self) {
        // A part it gave, counted given, saved no GET.
        self.health.counters.given.fetch_sub(1, Ordering::Relaxed);
        self.health.down("a part it gave failed its CRC");
    }
}

/// The owner of part `part` of the object at `key` among the hosts at `addresses`: the one whose
/// address, hashed with the key and the part's number, scores highest, ties going to the address
/// sorted last. None if there are none.
pub fn owner_of<'a>(
    addresses: impl IntoIterator<Item = &'a str>,
    key: &str,
    part: u64,
) -> Option<&'a str> {
    addresses.into_iter().max_by_key(|address| (score(address, key, part), *address))
}

/// The score of the host at `address` for part `part` of the object at `key`: SipHash-1-3, of fixed
/// keys, which hosts of any build compute alike, of the three, each length-prefixed.
fn score(address: &str, key: &str, part: u64) -> u64 {
    let mut hasher = siphasher::sip::SipHasher13::new_with_keys(0, 0);
    for bytes in [address.as_bytes(), key.as_bytes()] {
        hasher.write_u64(bytes.len() as u64);
        hasher.write(bytes);
    }
    hasher.write_u64(part);
    hasher.finish()
}

#[cfg(test)]
pub(crate) mod tests;
