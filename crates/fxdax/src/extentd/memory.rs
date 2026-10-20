// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The daemon's memory, which its cached extents' pages, and its stores' open extents', are of
//! (TASK-50.3): the memory a tenant holds, its arena's, and every tenant's together, each limited
//! at runtime through the control API. Past a limit the daemon evicts the clean pages asked for
//! longest ago, the tenant's own past the tenant's, any tenant's past the total; with none left to
//! evict, what would take more fails with `ENOMEM` (`crate::extents`' relief), and the daemon goes
//! on. Its metrics name tenants by [`label`]s.

use super::server::Tenant;
use crate::Errno;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use zircon::vm::physmap::CHUNK_SIZE;
use zircon::vm::pmm;

/// The tenants alive, and their limits, under one lock, so that a tenant made as its limit is set
/// is limited as it was set.
static REGISTRY: Mutex<Registry> = Mutex::new(Registry { tenants: Vec::new(), limits: None });

struct Registry {
    /// The tenants alive, which may outlive their grants, while their VM processes do: a tenant
    /// revoked and granted again is two at once for a while, each limited.
    tenants: Vec<Weak<Tenant>>,
    /// The tenants' limits, in bytes, by name: those granted later, or again, are limited as they
    /// are made.
    limits: Option<HashMap<String, u64>>,
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

/// Counts `tenant`, new, among those alive, limited as its name is.
pub(super) fn register(tenant: &Arc<Tenant>) {
    let mut registry = REGISTRY.lock().unwrap();
    if let Some(bytes) = registry.limit(tenant.name()) {
        pmm::node().set_arena_cap(tenant.arena(), Some(bytes));
    }
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
    let node = pmm::node();
    let Some(name) = tenant else {
        node.set_total_cap(bytes);
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
        node.set_arena_cap(tenant.arena(), bytes);
    }
    Ok(())
}

/// Evicts clean pages, those asked for longest ago first, of the tenant `tenant`'s, or of any, until
/// it, or the daemon, holds at most `target` bytes, or nothing is left to evict; and returns what it
/// holds then. `ENOENT` if no tenant `tenant` is alive. Memory is held in chunks of 2 MiB, which go
/// back once the last of their pages does.
pub fn shrink(tenant: Option<&str>, target: u64) -> Result<u64, Errno> {
    let node = pmm::node();
    let Some(name) = tenant else {
        // Free memory first: the arenas' spare chunks.
        node.release_spares(None);
        crate::extents::evict_least_used(None, |_| node.arenas_bytes() <= target);
        return Ok(node.arenas_bytes());
    };
    let arenas: Vec<_> = (REGISTRY.lock().unwrap().alive())
        .filter(|tenant| tenant.name() == name)
        .map(|tenant| tenant.arena())
        .collect();
    if arenas.is_empty() {
        return Err(Errno::ENOENT);
    }
    let held = || arenas.iter().map(|arena| node.arena_bytes(*arena)).sum::<u64>();
    for arena in &arenas {
        node.release_spares(Some(*arena));
        crate::extents::evict_least_used(Some(*arena), |_| held() <= target);
    }
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

/// The memory of each tenant alive, by label: a tenant alive twice, revoked and granted again, is
/// counted once, with both its arenas'.
pub fn tenants() -> Vec<TenantMemory> {
    let node = pmm::node();
    let registry = REGISTRY.lock().unwrap();
    let mut memory: HashMap<String, TenantMemory> = HashMap::new();
    for tenant in registry.alive() {
        let entry = memory.entry(tenant.name().to_owned()).or_insert_with(|| TenantMemory {
            label: label(tenant.name()),
            bytes: 0,
            limit: registry.limit(tenant.name()),
        });
        entry.bytes += node.arena_bytes(tenant.arena());
    }
    let mut memory: Vec<_> = memory.into_values().collect();
    memory.sort_by(|a, b| a.label.cmp(&b.label));
    memory
}
