# Integrator guide: running tenants' VMs on extentd

For whoever writes the VM manager, the trusted service on each host that talks to the extentd
daemon and starts Firecracker. `crates/fxdax/proto/fxdax/extentd/v1/extentd.proto` is the reference
for every call named here.

The VM manager is trusted. VM processes are not: treat one as compromised, and everything here
follows from that.

## Per-host setup

1. Start `fxdax-extentd` with `FXDAX_EXTENTD_GRPC` naming its control socket. Keep that socket away
   from jails, and set the daemon's umask so that only the VM manager can reach it: it can do
   everything, and unlike the VM socket it follows the umask (TASK-159).
2. For each tenant, call `Grant` with the tenant's name, the directory of its stores, and its jails'
   uid and gid. It returns the path of a socket.
3. Bind-mount **that socket alone** into the tenant's jails. Never mount the daemon's default VM
   socket into a jail: it can take any branch by its ID.
4. Run jails with Firecracker's jailer, as a uid other than the daemon's. The default VM socket is
   mode `0600`, owned by the daemon, so that a mistake in step 3 still can't connect.

Grants don't survive a restart of the daemon. After one, call `Grant` again, and bind-mount the new
socket.

## Restoring a tenant's VM on another host

A VM process can't name a branch: extentd ignores what it says, or any stolen ID would take another
tenant's data. The VM manager puts the snapshot's branch in the tenant's directory instead, and that
is the grant.

Without step 2 below, the restore fails with `ENOENT`.

### 1. On the snapshot's host, once it's taken

```
SyncStore(point)
branch = StatStore(point).branch      # keep this with the snapshot
```

Use this `branch`. **Never use `point_branch` from the snapshot file: a VM process wrote it.**

### 2. On the target host, before `PUT /snapshot/load`

```
CreateStore(path = <tenant's directory>/<point>, capacity, branch, token?)
```

| Result | Meaning | Do |
|---|---|---|
| OK | The store is made. The daemon takes the branch at the first restore | Go on |
| `ALREADY_EXISTS` | A store is there, perhaps a VM process's | Use it only if `StatStore(path).branch == branch`. Otherwise stop |
| `FAILED_PRECONDITION`, `ESTALE` | The branch's last writer was placed later than this host | Retry with `token` from a placement no earlier than that writer's (`PlaceStore`) |

Only do this for the tenant whose snapshot it is. Its VM processes can open that point and write it.

### 3. Load the snapshot

In `extent_overrides`:

| Field | Value |
|---|---|
| `point_on_host` | The path from step 2 |
| `path_on_host` | The VM's store, in the tenant's directory |
| `extentd_socket` | The tenant's socket, as the jail sees it |

Each restore forks the point.

### 4. When this host won't restore that snapshot any more

```
DropStore(point)
```

**Never `DeleteStore`**: that deletes the branch, which is the snapshot, on every host. `DropStore`
fails with `FAILED_PRECONDITION` (`EBUSY`) while a VM process has the store open. It removes this
host's copy only, so run `SyncStore` first if it has commits to keep.

`fxdaxctl drop DIR` does the same from a shell.

## Limits

**One host at a time.** A host's copy of a point is a writer of its branch, and a restore's fork is
a commit there. Once one host forks it, every other host's copy, the snapshot's own included, fails
with `EIO`. To restore on another host: `DropStore` here, then `CreateStore` there. Restoring one
snapshot on several hosts at once isn't supported yet (TASK-158).

**Memory.** Open pages can't be evicted until a guest seals them, and only a guest can seal. Several
VMs share the daemon's memory, so one idle VM with many open pages can leave another short, which
it sees as `ENOMEM`. This hasn't been run with several VMs at once.
