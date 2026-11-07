// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The daemon's memory, which its cached extents' pages are of (TASK-50.3): the memory a tenant
//! holds, its arena's, and every tenant's together, each limited at runtime through the control
//! API. Past a limit the daemon evicts the clean pages asked for longest ago, the tenant's own
//! past the tenant's, any tenant's past the total; with none left to evict, what would take more
//! fails with `ENOMEM` (physical memory's relief, with `crate::cache`'s evictor), and the daemon
//! goes on. fxdax: the private pages of a window that copies on write, its VM process's memory,
//! are reserved against its tenant's as the window is made (`physmem::pin`), so that relief
//! keeps room for them (backlog doc-13). Its metrics name tenants by [`label`]s.

use super::server::{Parking, Tenant};
use crate::Errno;
use physmem::CHUNK_SIZE;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// The tenants alive, and their limits, under one lock, so that a tenant made as its limit is set
/// is limited as it was set.
static REGISTRY: Mutex<Registry> =
    Mutex::new(Registry { tenants: Vec::new(), limits: None, rooms: None });

struct Registry {
    /// The tenants alive, which may outlive their grants, while their VM processes do: a tenant
    /// revoked and granted again goes on as it was ([`alive`]), but with another directory, when
    /// it is two at once for a while, each limited.
    tenants: Vec<Weak<Tenant>>,
    /// The tenants' limits, in bytes, by name: those granted later, or again, are limited as they
    /// are made.
    limits: Option<HashMap<String, u64>>,
    /// fxdax: the tenants' rooms ([`InFlight`]), in bytes, by name, as their limits are.
    rooms: Option<HashMap<String, u64>>,
}

impl Registry {
    fn alive(&self) -> impl Iterator<Item = Arc<Tenant>> + '_ {
        self.tenants.iter().filter_map(Weak::upgrade)
    }

    fn limit(&self, name: &str) -> Option<u64> {
        self.limits.as_ref().and_then(|limits| limits.get(name).copied())
    }
}

/// A tenant's label in the metrics: the first 16 hexadecimal digits of the SHA-256 of its name,
/// as `printf %s NAME | sha256sum | cut -c1-16` prints them, so that metrics, scraped elsewhere,
/// name no tenant, and an operator who knows its name finds it.
pub fn label(name: &str) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(name.as_bytes());
    digest[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The tenant named `name`, whose stores are in `dir`, alive, if one is: one revoked whose VM
/// processes live on, which a grant of it again goes on with (TASK-79.36).
pub(super) fn alive(name: &str, dir: &std::path::Path) -> Option<Arc<Tenant>> {
    let registry = REGISTRY.lock().unwrap();
    registry.alive().find(|tenant| tenant.name() == name && tenant.dir() == Some(dir))
}

/// Counts `tenant`, new, among those alive, limited as its name is.
pub(super) fn register(tenant: &Arc<Tenant>) {
    let mut registry = REGISTRY.lock().unwrap();
    if let Some(bytes) = registry.limit(tenant.name()) {
        physmem::set_arena_cap(tenant.arena(), Some(bytes));
    }
    let room = registry.rooms.as_ref().and_then(|rooms| rooms.get(tenant.name()).copied());
    tenant.in_flight().set_room(room);
    registry.tenants.retain(|alive| alive.strong_count() > 0);
    registry.tenants.push(Arc::downgrade(tenant));
}

/// Limits the memory of the tenant `tenant` to `bytes`, rounded down to 2 MiB, or every tenant's
/// together, without one; or lifts the limit, without `bytes`. A tenant's limit holds for it alive
/// now, and granted later, until it is set again. `EINVAL` for a limit under 2 MiB, which would
/// refuse every page.
pub fn set_limit(tenant: Option<&str>, bytes: Option<u64>) -> Result<(), Errno> {
    if bytes.is_some_and(|bytes| bytes < CHUNK_SIZE as u64) {
        return Err(Errno::EINVAL);
    }
    let Some(name) = tenant else {
        physmem::set_total_cap(bytes);
        return Ok(());
    };
    super::grants::check_name(name)?;
    let mut registry = REGISTRY.lock().unwrap();
    let limits = registry.limits.get_or_insert_with(HashMap::new);
    match bytes {
        Some(bytes) => limits.insert(name.to_owned(), bytes),
        None => limits.remove(name),
    };
    for tenant in registry.alive().filter(|tenant| tenant.name() == name) {
        physmem::set_arena_cap(tenant.arena(), bytes);
    }
    Ok(())
}

/// Evicts clean pages, those asked for longest ago first, of the tenant `tenant`'s, or of any, until
/// it, or the daemon, holds at most `target` bytes, or nothing is left to evict; and returns what it
/// holds then. `ENOENT` if no tenant `tenant` is alive. Memory is held in chunks of 2 MiB, which go
/// back once the last of their pages does. fxdax: a page a VM process's window maps, evicted, is
/// free once the VM process has unmapped it, or the daemon let it go (TASK-79.9): each extent's
/// pages free before what is held is looked at again ([`Parking`]), so that it evicts no more than
/// it must, and returns what is held once they are, not before.
pub fn shrink(tenant: Option<&str>, target: u64) -> Result<u64, Errno> {
    let parking = Parking::start();
    let Some(name) = tenant else {
        // Free memory first: the arenas' spare chunks.
        physmem::release_spares(None);
        crate::cache::evict_least_used(None, |_| {
            parking.wait();
            physmem::arenas_bytes() <= target
        });
        parking.wait();
        return Ok(physmem::arenas_bytes());
    };
    let arenas: Vec<_> = (REGISTRY.lock().unwrap().alive())
        .filter(|tenant| tenant.name() == name)
        .map(|tenant| tenant.arena())
        .collect();
    if arenas.is_empty() {
        return Err(Errno::ENOENT);
    }
    let held = || arenas.iter().map(|arena| physmem::arena_bytes(*arena)).sum::<u64>();
    for arena in &arenas {
        physmem::release_spares(Some(*arena));
        crate::cache::evict_least_used(Some(*arena), |_| {
            parking.wait();
            held() <= target
        });
    }
    parking.wait();
    Ok(held())
}

/// A tenant's memory, as the metrics show it.
pub struct TenantMemory {
    /// Its [`label`].
    pub label: String,
    /// What its arena holds, in bytes.
    pub bytes: u64,
    /// Its limit, if it has one.
    pub limit: Option<u64>,
}

/// The memory of each tenant alive, by label: a tenant alive twice, revoked and granted again with
/// another directory, is counted once, with both its arenas'.
pub fn tenants() -> Vec<TenantMemory> {
    let registry = REGISTRY.lock().unwrap();
    let mut memory: HashMap<String, TenantMemory> = HashMap::new();
    for tenant in registry.alive() {
        let entry = memory.entry(tenant.name().to_owned()).or_insert_with(|| TenantMemory {
            label: label(tenant.name()),
            bytes: 0,
            limit: registry.limit(tenant.name()),
        });
        entry.bytes += physmem::arena_bytes(tenant.arena());
    }
    let mut memory: Vec<_> = memory.into_values().collect();
    memory.sort_by(|a, b| a.label.cmp(&b.label));
    memory
}

/// fxdax: the room a tenant's connections share, unless one is set for it ([`set_room`]):
/// what of its VM processes' requests the daemon holds at once, their stream data and what it has
/// queued for them, together (TASK-149.35). Each connection has a room of its own too
/// (`server::Peer::room`, 272 MiB without a window), and a tenant as many as 1,024 connections:
/// some 272 GiB of one tenant's, held within the protocol, every tenant's daemon with it. A VM
/// process moves the data of its requests through its transfer area, up to 256 KiB each, 8 at once,
/// which no room counts; past that, through the stream. 8 GiB is 30 VM processes at once keeping
/// their rooms full, which none does but writing 64 MiB extents back to back, where a host's
/// density case, 300 forks reading their base, held 16 MiB of the tenant's at most.
pub const DEFAULT_TENANT_ROOM: u64 = 8 << 30;

/// fxdax: the room of every tenant whose own isn't set ([`set_room`]).
static TENANT_ROOM: AtomicU64 = AtomicU64::new(DEFAULT_TENANT_ROOM);

/// fxdax: sets the room the connections of the tenant `tenant` share to `bytes`, or, without
/// `bytes`, to every tenant's; or, without `tenant`, every tenant's whose own isn't set, the
/// default's without `bytes` (TASK-149.35). A tenant's holds for it alive now, and granted later,
/// until it is set again. `EINVAL` for a room under a payload's most, which a request of it
/// couldn't be held in.
pub fn set_room(tenant: Option<&str>, bytes: Option<u64>) -> Result<(), Errno> {
    if bytes.is_some_and(|bytes| bytes < super::wire::MAX_PAYLOAD) {
        return Err(Errno::EINVAL);
    }
    let Some(name) = tenant else {
        TENANT_ROOM.store(bytes.unwrap_or(DEFAULT_TENANT_ROOM), Ordering::Relaxed);
        // Larger, it lets those waiting go on.
        let registry = REGISTRY.lock().unwrap();
        registry.alive().for_each(|tenant| tenant.in_flight().changed());
        if let Ok(tenant) = super::server::default_tenant() {
            tenant.in_flight().changed();
        }
        return Ok(());
    };
    super::grants::check_name(name)?;
    let mut registry = REGISTRY.lock().unwrap();
    let rooms = registry.rooms.get_or_insert_with(HashMap::new);
    match bytes {
        Some(bytes) => rooms.insert(name.to_owned(), bytes),
        None => rooms.remove(name),
    };
    for tenant in registry.alive().filter(|tenant| tenant.name() == name) {
        tenant.in_flight().set_room(bytes);
    }
    Ok(())
}

/// fxdax: what a tenant's connections have the daemon hold of its VM processes' requests together
/// (TASK-149.35): what each counts against its own room, its requests' stream data and what it
/// queued for its VM process, within the room they share, the tenant's own or every tenant's
/// ([`DEFAULT_TENANT_ROOM`]). A request past it waits, before its payload is read, until what is
/// held leaves it room; or, with nothing held, goes on, so that one as large as the room is
/// served. Not let go, as a connection past its own room is: it keeps within its own, and the
/// tenant's other VM processes, a healthy tenant's busy ones, take theirs.
#[derive(Default)]
pub struct InFlight {
    held: AtomicU64,
    /// Its room, or 0, every tenant's ([`TENANT_ROOM`]).
    room: AtomicU64,
    /// The most it held, which the tests measure.
    peak: AtomicU64,
    /// What says it held less, or its room grew.
    less: event_listener::Event,
}

impl InFlight {
    /// What it holds.
    pub fn held(&self) -> u64 {
        self.held.load(Ordering::Relaxed)
    }

    /// The most it held.
    pub fn peak(&self) -> u64 {
        self.peak.load(Ordering::Relaxed)
    }

    /// Its room: its own, or every tenant's.
    pub fn room(&self) -> u64 {
        match self.room.load(Ordering::Relaxed) {
            0 => TENANT_ROOM.load(Ordering::Relaxed),
            room => room,
        }
    }

    fn set_room(&self, bytes: Option<u64>) {
        self.room.store(bytes.unwrap_or(0), Ordering::Relaxed);
        self.changed();
    }

    /// Tells those waiting for room that there may be some.
    fn changed(&self) {
        self.less.notify(usize::MAX);
    }

    /// Holds `bytes` more, room or not: what the daemon queued for a VM process, which it holds
    /// whatever.
    pub fn add(&self, bytes: u64) {
        let held = self.held.fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.peak.fetch_max(held, Ordering::Relaxed);
    }

    /// Holds `bytes` fewer.
    pub fn sub(&self, bytes: u64) {
        if bytes > 0 {
            self.held.fetch_sub(bytes, Ordering::Relaxed);
            self.changed();
        }
    }

    /// Holds `bytes` more, if they are within its room, or it holds nothing.
    fn try_take(&self, bytes: u64) -> bool {
        let room = self.room();
        let taken = self.held.try_update(Ordering::Relaxed, Ordering::Relaxed, |held| {
            (held == 0 || held + bytes <= room).then_some(held + bytes)
        });
        if let Ok(held) = taken {
            self.peak.fetch_max(held + bytes, Ordering::Relaxed);
        }
        taken.is_ok()
    }

    /// Holds `bytes` more, once they are within its room, or it holds nothing: waiting, holding no
    /// thread, until then.
    pub async fn take(&self, bytes: u64) {
        loop {
            if self.try_take(bytes) {
                return;
            }
            let less = self.less.listen();
            if self.try_take(bytes) {
                return;
            }
            less.await;
        }
    }
}
