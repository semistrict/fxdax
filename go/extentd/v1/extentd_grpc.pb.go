// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The extent daemon's control API (TASK-43.3): what the host's VM manager, which launches the
// VMMs, asks fxdax-extentd to do with the VMs' disks, the extent stores it serves them. It is
// served over a Unix socket (`FXDAX_EXTENTD_GRPC`), the daemon's user's alone, mode 0600 whatever
// its umask (TASK-159), as every caller may do everything. VM processes do not use it: they keep
// the extent protocol on the daemon's VM sockets, which carries their memory as file descriptors.
//
// Tenants (Grant, Revoke) are defence in depth, should a guest escape into its VMM: a tenant's
// VM processes connect to the tenant's own socket, through which they name stores in the
// tenant's directory alone, and hold the tenant's cached pages alone, with public bases' pages
// read-only. Guest isolation is the security boundary, which nothing relies on tenants for: every
// VM may as well be of one tenant, or of none, on the daemon's own VM socket. Grants are not kept
// across restarts of the daemon: the caller grants again.
//
// A snapshot's point is immutable (TASK-158): any number of hosts read it, none writes it, it has
// no lease, and a restore's fork is a new branch's record alone, so that a restore on one host
// changes nothing another sees, and no host's copy goes stale.
//
// A tenant's restore on a host without the point (TASK-154). A VM process names no branch: what a
// tenant's says in OPEN is ignored, as anyone's ID would take that branch's data. A tenant's stores
// are what is in its directory, which only this API puts a branch of the bucket in, so that the
// caller's CreateStore of a branch in a tenant's directory grants it, and DropStore ends that. A
// snapshot of a tenant's VM restores so, on any number of hosts at once:
//
// - On the snapshot's host, once it is taken: StatStore's `branch` of its point, kept with the
//   snapshot. The point is in the bucket by then. Not the snapshot file's `point_branch`, which a
//   VM process wrote. The daemon's is sound: a fork to an ID another branch has fails
//   (ALREADY_EXISTS).
// - On each host to restore it, once, before the VMM's first `PUT /snapshot/load`: CreateStore at a
//   path in the tenant's directory, of that branch, which that host's daemon reads as the first
//   restore forks it. ALREADY_EXISTS means a store is there, a VM process's perhaps: use it only if
//   StatStore's `branch` is the branch. A point needs no `token`.
// - Then each load, whose `extent_overrides` name that path as `point_on_host`, the VM's store in
//   the tenant's directory as `path_on_host`, and the tenant's socket as `extentd_socket`. Each
//   restore forks the point, on as many hosts at once as there are. Without the CreateStore, it
//   fails with ENOENT, whatever the VMM names; so does one of a point deleted since.
// - Once this host restores it no more: DropStore of the point, optional, which ends the grant here
//   and frees the copy. DeleteStore deletes the point, which is the snapshot, on every host: VMs
//   restored from it keep what they inherited.
//
// A branch so granted is the tenant's to read and restore: a VM process opening a point to write
// it fails with EROFS. Grant one only to the tenant whose snapshot it is.
//
// Shared points (TASK-395.7): a point forks only into cells of its own tenant's, unless it is
// shared (SharePoint, or ForkStore's `shared`): a base image or a snapshot that many tenants clone.
// A restore, or a ForkStore, of a point of another tenant's that isn't shared fails with
// PERMISSION_DENIED (EACCES), making nothing; each fork reads whether the point is shared, in the
// bucket, as it is made, so that unsharing it stops new forks on every host, and those made live
// on. A store with a writer forks into cells of other tenants' only if it is tenant 0's, as the
// daemon's own stores and public bases are (TASK-395.9): a ForkStore of another tenant's into
// another tenant's directory fails with PERMISSION_DENIED (EACCES), making nothing.
//
// A fork of anything but a point is a commit of its branch, which takes its branch's writer: a
// store forks on one host at a time. A base, or any store, forked on several hosts is made a point
// first, by ForkStore's `point`, and forked as a snapshot's is, from a copy on each host.
//
// Stores are named by the paths of their directories on the daemon's host, UTF-8. A store a VM
// process has open is its connection's: what would change it under the guest's feet fails with
// FAILED_PRECONDITION and errno EBUSY, unless the call says otherwise.
//
// Errors are gRPC statuses whose code follows the errno the daemon failed with, which the
// `fxdax-errno` metadata entry carries, as a decimal number (Linux's): ENOENT is NOT_FOUND,
// EEXIST ALREADY_EXISTS, EACCES and EPERM PERMISSION_DENIED, EINVAL INVALID_ARGUMENT, ENOSPC
// RESOURCE_EXHAUSTED, ETIMEDOUT DEADLINE_EXCEEDED, EBUSY, ESTALE, ENOTSUP and EROFS
// FAILED_PRECONDITION, EIO and others INTERNAL.
//
// Calls block for as long as their work takes: a sync until the store's commits are in object
// storage, an import for minutes. A caller bounds them with deadlines; one that gives up does not
// stop the work, which completes, or fails, on its own.
//
// Compatibility (TASK-50.5): v1 only grows. Calls and fields are added; none is removed,
// renamed, renumbered or given another meaning or type, and no field number is used twice (a
// field that goes is `reserved`). A field a daemon doesn't know of is zero, empty or false, which
// each says it means; a caller ignores fields it doesn't know of. A call an older daemon lacks
// fails with UNIMPLEMENTED. Errnos keep their codes. A change that breaks a caller is v2, served
// beside v1 until callers move.
//
// Cells (TASK-364): every branch is a volume of a tenant's cell, kept under the cell's prefix in
// the bucket, and a host takes a cell whole, its volumes' leases with it. A branch is named by its
// tenant, its cell and its ID: `tenant`, `cell` and `branch` beside one another. Tenants and cells
// are IDs alone, which the caller keeps the meaning of. A granted tenant's ID is its grant's
// (Grant's `id`); the daemon's own socket's VM processes make stores of tenant 0.

// Code generated by protoc-gen-go-grpc. DO NOT EDIT.
// versions:
// - protoc-gen-go-grpc v1.6.2
// - protoc             (unknown)
// source: fxdax/extentd/v1/extentd.proto

package extentdv1

import (
	context "context"
	grpc "google.golang.org/grpc"
	codes "google.golang.org/grpc/codes"
	status "google.golang.org/grpc/status"
)

// This is a compile-time assertion to ensure that this generated file
// is compatible with the grpc package it is being compiled against.
// Requires gRPC-Go v1.64.0 or later.
const _ = grpc.SupportPackageIsVersion9

const (
	Extentd_CreateStore_FullMethodName     = "/fxdax.extentd.v1.Extentd/CreateStore"
	Extentd_ImportImage_FullMethodName     = "/fxdax.extentd.v1.Extentd/ImportImage"
	Extentd_ForkStore_FullMethodName       = "/fxdax.extentd.v1.Extentd/ForkStore"
	Extentd_DeleteStore_FullMethodName     = "/fxdax.extentd.v1.Extentd/DeleteStore"
	Extentd_DropStore_FullMethodName       = "/fxdax.extentd.v1.Extentd/DropStore"
	Extentd_ResizeStore_FullMethodName     = "/fxdax.extentd.v1.Extentd/ResizeStore"
	Extentd_PlaceStore_FullMethodName      = "/fxdax.extentd.v1.Extentd/PlaceStore"
	Extentd_MakePublic_FullMethodName      = "/fxdax.extentd.v1.Extentd/MakePublic"
	Extentd_SharePoint_FullMethodName      = "/fxdax.extentd.v1.Extentd/SharePoint"
	Extentd_AttachBase_FullMethodName      = "/fxdax.extentd.v1.Extentd/AttachBase"
	Extentd_SyncStore_FullMethodName       = "/fxdax.extentd.v1.Extentd/SyncStore"
	Extentd_StatStore_FullMethodName       = "/fxdax.extentd.v1.Extentd/StatStore"
	Extentd_ListStores_FullMethodName      = "/fxdax.extentd.v1.Extentd/ListStores"
	Extentd_GetStats_FullMethodName        = "/fxdax.extentd.v1.Extentd/GetStats"
	Extentd_GetSnapshot_FullMethodName     = "/fxdax.extentd.v1.Extentd/GetSnapshot"
	Extentd_Grant_FullMethodName           = "/fxdax.extentd.v1.Extentd/Grant"
	Extentd_Revoke_FullMethodName          = "/fxdax.extentd.v1.Extentd/Revoke"
	Extentd_SetMemoryLimit_FullMethodName  = "/fxdax.extentd.v1.Extentd/SetMemoryLimit"
	Extentd_SetMemoryWeight_FullMethodName = "/fxdax.extentd.v1.Extentd/SetMemoryWeight"
	Extentd_SetTenantRoom_FullMethodName   = "/fxdax.extentd.v1.Extentd/SetTenantRoom"
	Extentd_ShrinkMemory_FullMethodName    = "/fxdax.extentd.v1.Extentd/ShrinkMemory"
	Extentd_SetDiskLimit_FullMethodName    = "/fxdax.extentd.v1.Extentd/SetDiskLimit"
	Extentd_GetTenantDisk_FullMethodName   = "/fxdax.extentd.v1.Extentd/GetTenantDisk"
	Extentd_SetPeers_FullMethodName        = "/fxdax.extentd.v1.Extentd/SetPeers"
)

// ExtentdClient is the client API for Extentd service.
//
// For semantics around ctx use and closing/ending streaming RPCs, please refer to https://pkg.go.dev/google.golang.org/grpc/?tab=doc#ClientConn.NewStream.
//
// The extent daemon's control API.
type ExtentdClient interface {
	// Makes an empty store: of a new branch; or of a branch kept in the daemon's bucket, which the
	// daemon takes once it opens the store (a VM on another host than the branch's), or reads, if it
	// is a point, a snapshot's or ForkStore's, which any number of hosts hold at once.
	CreateStore(ctx context.Context, in *CreateStoreRequest, opts ...grpc.CallOption) (*CreateStoreResponse, error)
	// Makes a store holding an Fxfs filesystem of a container image: a base for VMs to fork.
	ImportImage(ctx context.Context, in *ImportImageRequest, opts ...grpc.CallOption) (*ImportImageResponse, error)
	// Forks a store to a new branch, a new store sharing its data: live, if a VM has it open, at a
	// point nothing is open on it (as with its guest's filesystem frozen), FAILED_PRECONDITION
	// (EBUSY) otherwise. A fork of a store with a writer is a commit of its branch, made where that
	// writer is, one host at a time; a fork of a point is its child's record alone, made on any
	// number of hosts at once. So a base forked on many hosts is made a point first (`point`).
	ForkStore(ctx context.Context, in *ForkStoreRequest, opts ...grpc.CallOption) (*ForkStoreResponse, error)
	// Deletes a store, or a fork, and its branch in the daemon's bucket: its data goes but for what
	// other branches share of it, which stays until the last of them goes. A snapshot's point deleted
	// restores no more, on any host (NOT_FOUND), and what was restored from it lives on.
	DeleteStore(ctx context.Context, in *DeleteStoreRequest, opts ...grpc.CallOption) (*DeleteStoreResponse, error)
	// Deletes this host's copy of a store, or a fork, as DeleteStore does, but for its branch in the
	// daemon's bucket, which stays, with what else holds it: a point this host read for a tenant,
	// which that tenant's VM processes restore no more here, its snapshot living on. A store whose
	// forks live here keeps its directory, which no longer opens, and takes no CreateStore until the
	// last of them goes. What its commits hold that the bucket doesn't goes with it: SyncStore first,
	// to keep it. FAILED_PRECONDITION (EBUSY) while a VM process has it open.
	DropStore(ctx context.Context, in *DropStoreRequest, opts ...grpc.CallOption) (*DropStoreResponse, error)
	// Sets a store's capacity, the size of the disk its guest sees: a store a VM has open grows at
	// once, the guest seeing it mounted, and shrinks if its guest's filesystem agrees, no lower than
	// what it holds (RESOURCE_EXHAUSTED otherwise); one none has open only grows. A point, which
	// nothing writes, is resized nowhere (FAILED_PRECONDITION, EROFS).
	ResizeStore(ctx context.Context, in *ResizeStoreRequest, opts ...grpc.CallOption) (*ResizeStoreResponse, error)
	// Places a store at a placement whose fencing token, its boot ID, is `token`: its writer takes
	// its branch in object storage with that token from then on, which a later placement's exceeds,
	// so that a placement cut off takes no branch back from the one after it.
	PlaceStore(ctx context.Context, in *PlaceStoreRequest, opts ...grpc.CallOption) (*PlaceStoreResponse, error)
	// Makes a store a public base, and the branches it inherited from: their pages are cached once,
	// in memory every tenant's VM processes map read-only, and shared by every tenant's forks.
	MakePublic(ctx context.Context, in *MakePublicRequest, opts ...grpc.CallOption) (*MakePublicResponse, error)
	// Shares a point, or no longer (TASK-395.7): while shared, it forks into cells of other tenants'
	// than its own, as a base image or a snapshot many tenants clone; unshared, such forks fail
	// (PERMISSION_DENIED, EACCES), and those made live on. A store of no point is refused
	// (INVALID_ARGUMENT, EINVAL); one of a point deleted, or none, NOT_FOUND (ENOENT). Nothing of the
	// store on this host changes: the point's record in the daemon's bucket says it, for every host.
	SharePoint(ctx context.Context, in *SharePointRequest, opts ...grpc.CallOption) (*SharePointResponse, error)
	// Attaches a fork of a store to a store, a VM's or stopped, and so to its forks and the VMs
	// restored from them: reads and maps of the fork's extents the store doesn't hold read through
	// to it, read-only. Everything the fork holds is readable to the guest, which names extents, so
	// that a store that isn't public attaches only to a store of its tenant (PERMISSION_DENIED
	// otherwise). The fork stays until the caller deletes it: a VM with it open keeps it from going
	// (FAILED_PRECONDITION), but a stopped store doesn't. No guest can ask for this.
	AttachBase(ctx context.Context, in *AttachBaseRequest, opts ...grpc.CallOption) (*AttachBaseResponse, error)
	// Returns once every commit of a store, as of the call, is durable and in the daemon's bucket,
	// as before a clean stop.
	SyncStore(ctx context.Context, in *SyncStoreRequest, opts ...grpc.CallOption) (*SyncStoreResponse, error)
	// What a store is, and holds.
	StatStore(ctx context.Context, in *StatStoreRequest, opts ...grpc.CallOption) (*StatStoreResponse, error)
	// Lists the stores the daemon serves: those VM processes have open, as writers or followers,
	// with what StatStore says of each (TASK-50.5).
	ListStores(ctx context.Context, in *ListStoresRequest, opts ...grpc.CallOption) (*ListStoresResponse, error)
	// What the daemon did since it started: counters, which only grow.
	GetStats(ctx context.Context, in *GetStatsRequest, opts ...grpc.CallOption) (*GetStatsResponse, error)
	// What the daemon, its VM processes and their guests counted, as of one moment, each with the
	// commit it was built of (TASK-111.4): the daemon asks every VM process that opened a store, each
	// its guest, waits for their answers, a while at most, and reads its own counters then.
	GetSnapshot(ctx context.Context, in *GetSnapshotRequest, opts ...grpc.CallOption) (*GetSnapshotResponse, error)
	// Grants a tenant a VM socket, making the tenant if it is new: the path of a socket for the
	// caller to bind-mount into the tenant's jails, through which VM processes open stores in the
	// tenant's directory alone. Granting a tenant again returns its socket, given to the uid and gid
	// named; with another directory, ALREADY_EXISTS.
	Grant(ctx context.Context, in *GrantRequest, opts ...grpc.CallOption) (*GrantResponse, error)
	// Revokes a tenant's grant: its socket goes, and takes no more connections. Its VM processes
	// connected keep their stores until they go; then the tenant goes, and its cached pages.
	Revoke(ctx context.Context, in *RevokeRequest, opts ...grpc.CallOption) (*RevokeResponse, error)
	// Limits the memory the daemon holds extents' pages in, at once (TASK-50.3): a tenant's, or
	// every tenant's together. Past a tenant's limit, the daemon evicts the clean pages of its
	// extents asked for longest ago; past the total, any tenant's; with none left to evict, what
	// would take more fails, with ENOMEM, which the VM's guest sees as an I/O error, and the daemon
	// goes on. A limit under what is held holds as pages are freed: ShrinkMemory frees them now.
	SetMemoryLimit(ctx context.Context, in *SetMemoryLimitRequest, opts ...grpc.CallOption) (*SetMemoryLimitResponse, error)
	// Weighs a tenant's memory against the others' (TASK-358). The daemon's memory is a page cache:
	// as much of the host's as is free, less a headroom; as the host's free memory falls, the daemon
	// gives it back, evicting each tenant first down to its share by weight of what it may still
	// hold, a tenant within its share kept, then any, least recently used first. Every tenant weighs
	// 1 until set, such as to the memory its sandboxes are given.
	SetMemoryWeight(ctx context.Context, in *SetMemoryWeightRequest, opts ...grpc.CallOption) (*SetMemoryWeightResponse, error)
	// Sets the room a tenant's VM processes' connections share (TASK-149.35): what of their
	// requests the daemon holds at once, their data in the stream and what it queued for them, which
	// each connection's own room bounds too. A request past it waits for room. By default 8 GiB a
	// tenant, which a tenant's VMs reach only writing large extents through the stream, back to back.
	SetTenantRoom(ctx context.Context, in *SetTenantRoomRequest, opts ...grpc.CallOption) (*SetTenantRoomResponse, error)
	// Evicts clean pages, of extents asked for longest ago first, until the memory a tenant, or the
	// daemon, holds is at most a target, or none is left to evict: as the host's memory runs low.
	ShrinkMemory(ctx context.Context, in *ShrinkMemoryRequest, opts ...grpc.CallOption) (*ShrinkMemoryResponse, error)
	// Limits the host's disk a tenant takes, or every tenant together (TASK-94): what is in its
	// directory, its stores' files, each file once, the host's part cache no tenant's. Past a
	// tenant's limit, its VMs' writes, and the stores they make, fail with ENOSPC, which the VM's
	// guest sees as a full disk, and the daemon and other tenants' writes go on; past the total, any
	// tenant's do. Commits go on, and what leaves the disk (uploaded, freed, a store deleted) lets
	// the tenant write again. A limit under what is taken holds as data leaves.
	SetDiskLimit(ctx context.Context, in *SetDiskLimitRequest, opts ...grpc.CallOption) (*SetDiskLimitResponse, error)
	// What the host's disk a tenant takes, or every tenant together, counted now, and its limit.
	GetTenantDisk(ctx context.Context, in *GetTenantDiskRequest, opts ...grpc.CallOption) (*GetTenantDiskResponse, error)
	// Has the hosts sharing the daemon's bucket be these (TASK-406.2): its stores' fetchers ask the
	// host that owns a part, by rendezvous hashing of the object's key and the part's number over
	// them, for it before the bucket, and it serves them the parts it owns. Those it had are kept,
	// down or not. FAILED_PRECONDITION (ENOTSUP) if the daemon serves no peers (it was started
	// without FXDAX_EXTENTD_PEER_ADDRESS); INVALID_ARGUMENT (EINVAL) for an address that is no
	// HOST:PORT.
	SetPeers(ctx context.Context, in *SetPeersRequest, opts ...grpc.CallOption) (*SetPeersResponse, error)
}

type extentdClient struct {
	cc grpc.ClientConnInterface
}

func NewExtentdClient(cc grpc.ClientConnInterface) ExtentdClient {
	return &extentdClient{cc}
}

func (c *extentdClient) CreateStore(ctx context.Context, in *CreateStoreRequest, opts ...grpc.CallOption) (*CreateStoreResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(CreateStoreResponse)
	err := c.cc.Invoke(ctx, Extentd_CreateStore_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) ImportImage(ctx context.Context, in *ImportImageRequest, opts ...grpc.CallOption) (*ImportImageResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(ImportImageResponse)
	err := c.cc.Invoke(ctx, Extentd_ImportImage_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) ForkStore(ctx context.Context, in *ForkStoreRequest, opts ...grpc.CallOption) (*ForkStoreResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(ForkStoreResponse)
	err := c.cc.Invoke(ctx, Extentd_ForkStore_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) DeleteStore(ctx context.Context, in *DeleteStoreRequest, opts ...grpc.CallOption) (*DeleteStoreResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(DeleteStoreResponse)
	err := c.cc.Invoke(ctx, Extentd_DeleteStore_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) DropStore(ctx context.Context, in *DropStoreRequest, opts ...grpc.CallOption) (*DropStoreResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(DropStoreResponse)
	err := c.cc.Invoke(ctx, Extentd_DropStore_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) ResizeStore(ctx context.Context, in *ResizeStoreRequest, opts ...grpc.CallOption) (*ResizeStoreResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(ResizeStoreResponse)
	err := c.cc.Invoke(ctx, Extentd_ResizeStore_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) PlaceStore(ctx context.Context, in *PlaceStoreRequest, opts ...grpc.CallOption) (*PlaceStoreResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(PlaceStoreResponse)
	err := c.cc.Invoke(ctx, Extentd_PlaceStore_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) MakePublic(ctx context.Context, in *MakePublicRequest, opts ...grpc.CallOption) (*MakePublicResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(MakePublicResponse)
	err := c.cc.Invoke(ctx, Extentd_MakePublic_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) SharePoint(ctx context.Context, in *SharePointRequest, opts ...grpc.CallOption) (*SharePointResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(SharePointResponse)
	err := c.cc.Invoke(ctx, Extentd_SharePoint_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) AttachBase(ctx context.Context, in *AttachBaseRequest, opts ...grpc.CallOption) (*AttachBaseResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(AttachBaseResponse)
	err := c.cc.Invoke(ctx, Extentd_AttachBase_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) SyncStore(ctx context.Context, in *SyncStoreRequest, opts ...grpc.CallOption) (*SyncStoreResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(SyncStoreResponse)
	err := c.cc.Invoke(ctx, Extentd_SyncStore_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) StatStore(ctx context.Context, in *StatStoreRequest, opts ...grpc.CallOption) (*StatStoreResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(StatStoreResponse)
	err := c.cc.Invoke(ctx, Extentd_StatStore_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) ListStores(ctx context.Context, in *ListStoresRequest, opts ...grpc.CallOption) (*ListStoresResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(ListStoresResponse)
	err := c.cc.Invoke(ctx, Extentd_ListStores_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) GetStats(ctx context.Context, in *GetStatsRequest, opts ...grpc.CallOption) (*GetStatsResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(GetStatsResponse)
	err := c.cc.Invoke(ctx, Extentd_GetStats_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) GetSnapshot(ctx context.Context, in *GetSnapshotRequest, opts ...grpc.CallOption) (*GetSnapshotResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(GetSnapshotResponse)
	err := c.cc.Invoke(ctx, Extentd_GetSnapshot_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) Grant(ctx context.Context, in *GrantRequest, opts ...grpc.CallOption) (*GrantResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(GrantResponse)
	err := c.cc.Invoke(ctx, Extentd_Grant_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) Revoke(ctx context.Context, in *RevokeRequest, opts ...grpc.CallOption) (*RevokeResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(RevokeResponse)
	err := c.cc.Invoke(ctx, Extentd_Revoke_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) SetMemoryLimit(ctx context.Context, in *SetMemoryLimitRequest, opts ...grpc.CallOption) (*SetMemoryLimitResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(SetMemoryLimitResponse)
	err := c.cc.Invoke(ctx, Extentd_SetMemoryLimit_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) SetMemoryWeight(ctx context.Context, in *SetMemoryWeightRequest, opts ...grpc.CallOption) (*SetMemoryWeightResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(SetMemoryWeightResponse)
	err := c.cc.Invoke(ctx, Extentd_SetMemoryWeight_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) SetTenantRoom(ctx context.Context, in *SetTenantRoomRequest, opts ...grpc.CallOption) (*SetTenantRoomResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(SetTenantRoomResponse)
	err := c.cc.Invoke(ctx, Extentd_SetTenantRoom_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) ShrinkMemory(ctx context.Context, in *ShrinkMemoryRequest, opts ...grpc.CallOption) (*ShrinkMemoryResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(ShrinkMemoryResponse)
	err := c.cc.Invoke(ctx, Extentd_ShrinkMemory_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) SetDiskLimit(ctx context.Context, in *SetDiskLimitRequest, opts ...grpc.CallOption) (*SetDiskLimitResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(SetDiskLimitResponse)
	err := c.cc.Invoke(ctx, Extentd_SetDiskLimit_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) GetTenantDisk(ctx context.Context, in *GetTenantDiskRequest, opts ...grpc.CallOption) (*GetTenantDiskResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(GetTenantDiskResponse)
	err := c.cc.Invoke(ctx, Extentd_GetTenantDisk_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *extentdClient) SetPeers(ctx context.Context, in *SetPeersRequest, opts ...grpc.CallOption) (*SetPeersResponse, error) {
	cOpts := append([]grpc.CallOption{grpc.StaticMethod()}, opts...)
	out := new(SetPeersResponse)
	err := c.cc.Invoke(ctx, Extentd_SetPeers_FullMethodName, in, out, cOpts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

// ExtentdServer is the server API for Extentd service.
// All implementations must embed UnimplementedExtentdServer
// for forward compatibility.
//
// The extent daemon's control API.
type ExtentdServer interface {
	// Makes an empty store: of a new branch; or of a branch kept in the daemon's bucket, which the
	// daemon takes once it opens the store (a VM on another host than the branch's), or reads, if it
	// is a point, a snapshot's or ForkStore's, which any number of hosts hold at once.
	CreateStore(context.Context, *CreateStoreRequest) (*CreateStoreResponse, error)
	// Makes a store holding an Fxfs filesystem of a container image: a base for VMs to fork.
	ImportImage(context.Context, *ImportImageRequest) (*ImportImageResponse, error)
	// Forks a store to a new branch, a new store sharing its data: live, if a VM has it open, at a
	// point nothing is open on it (as with its guest's filesystem frozen), FAILED_PRECONDITION
	// (EBUSY) otherwise. A fork of a store with a writer is a commit of its branch, made where that
	// writer is, one host at a time; a fork of a point is its child's record alone, made on any
	// number of hosts at once. So a base forked on many hosts is made a point first (`point`).
	ForkStore(context.Context, *ForkStoreRequest) (*ForkStoreResponse, error)
	// Deletes a store, or a fork, and its branch in the daemon's bucket: its data goes but for what
	// other branches share of it, which stays until the last of them goes. A snapshot's point deleted
	// restores no more, on any host (NOT_FOUND), and what was restored from it lives on.
	DeleteStore(context.Context, *DeleteStoreRequest) (*DeleteStoreResponse, error)
	// Deletes this host's copy of a store, or a fork, as DeleteStore does, but for its branch in the
	// daemon's bucket, which stays, with what else holds it: a point this host read for a tenant,
	// which that tenant's VM processes restore no more here, its snapshot living on. A store whose
	// forks live here keeps its directory, which no longer opens, and takes no CreateStore until the
	// last of them goes. What its commits hold that the bucket doesn't goes with it: SyncStore first,
	// to keep it. FAILED_PRECONDITION (EBUSY) while a VM process has it open.
	DropStore(context.Context, *DropStoreRequest) (*DropStoreResponse, error)
	// Sets a store's capacity, the size of the disk its guest sees: a store a VM has open grows at
	// once, the guest seeing it mounted, and shrinks if its guest's filesystem agrees, no lower than
	// what it holds (RESOURCE_EXHAUSTED otherwise); one none has open only grows. A point, which
	// nothing writes, is resized nowhere (FAILED_PRECONDITION, EROFS).
	ResizeStore(context.Context, *ResizeStoreRequest) (*ResizeStoreResponse, error)
	// Places a store at a placement whose fencing token, its boot ID, is `token`: its writer takes
	// its branch in object storage with that token from then on, which a later placement's exceeds,
	// so that a placement cut off takes no branch back from the one after it.
	PlaceStore(context.Context, *PlaceStoreRequest) (*PlaceStoreResponse, error)
	// Makes a store a public base, and the branches it inherited from: their pages are cached once,
	// in memory every tenant's VM processes map read-only, and shared by every tenant's forks.
	MakePublic(context.Context, *MakePublicRequest) (*MakePublicResponse, error)
	// Shares a point, or no longer (TASK-395.7): while shared, it forks into cells of other tenants'
	// than its own, as a base image or a snapshot many tenants clone; unshared, such forks fail
	// (PERMISSION_DENIED, EACCES), and those made live on. A store of no point is refused
	// (INVALID_ARGUMENT, EINVAL); one of a point deleted, or none, NOT_FOUND (ENOENT). Nothing of the
	// store on this host changes: the point's record in the daemon's bucket says it, for every host.
	SharePoint(context.Context, *SharePointRequest) (*SharePointResponse, error)
	// Attaches a fork of a store to a store, a VM's or stopped, and so to its forks and the VMs
	// restored from them: reads and maps of the fork's extents the store doesn't hold read through
	// to it, read-only. Everything the fork holds is readable to the guest, which names extents, so
	// that a store that isn't public attaches only to a store of its tenant (PERMISSION_DENIED
	// otherwise). The fork stays until the caller deletes it: a VM with it open keeps it from going
	// (FAILED_PRECONDITION), but a stopped store doesn't. No guest can ask for this.
	AttachBase(context.Context, *AttachBaseRequest) (*AttachBaseResponse, error)
	// Returns once every commit of a store, as of the call, is durable and in the daemon's bucket,
	// as before a clean stop.
	SyncStore(context.Context, *SyncStoreRequest) (*SyncStoreResponse, error)
	// What a store is, and holds.
	StatStore(context.Context, *StatStoreRequest) (*StatStoreResponse, error)
	// Lists the stores the daemon serves: those VM processes have open, as writers or followers,
	// with what StatStore says of each (TASK-50.5).
	ListStores(context.Context, *ListStoresRequest) (*ListStoresResponse, error)
	// What the daemon did since it started: counters, which only grow.
	GetStats(context.Context, *GetStatsRequest) (*GetStatsResponse, error)
	// What the daemon, its VM processes and their guests counted, as of one moment, each with the
	// commit it was built of (TASK-111.4): the daemon asks every VM process that opened a store, each
	// its guest, waits for their answers, a while at most, and reads its own counters then.
	GetSnapshot(context.Context, *GetSnapshotRequest) (*GetSnapshotResponse, error)
	// Grants a tenant a VM socket, making the tenant if it is new: the path of a socket for the
	// caller to bind-mount into the tenant's jails, through which VM processes open stores in the
	// tenant's directory alone. Granting a tenant again returns its socket, given to the uid and gid
	// named; with another directory, ALREADY_EXISTS.
	Grant(context.Context, *GrantRequest) (*GrantResponse, error)
	// Revokes a tenant's grant: its socket goes, and takes no more connections. Its VM processes
	// connected keep their stores until they go; then the tenant goes, and its cached pages.
	Revoke(context.Context, *RevokeRequest) (*RevokeResponse, error)
	// Limits the memory the daemon holds extents' pages in, at once (TASK-50.3): a tenant's, or
	// every tenant's together. Past a tenant's limit, the daemon evicts the clean pages of its
	// extents asked for longest ago; past the total, any tenant's; with none left to evict, what
	// would take more fails, with ENOMEM, which the VM's guest sees as an I/O error, and the daemon
	// goes on. A limit under what is held holds as pages are freed: ShrinkMemory frees them now.
	SetMemoryLimit(context.Context, *SetMemoryLimitRequest) (*SetMemoryLimitResponse, error)
	// Weighs a tenant's memory against the others' (TASK-358). The daemon's memory is a page cache:
	// as much of the host's as is free, less a headroom; as the host's free memory falls, the daemon
	// gives it back, evicting each tenant first down to its share by weight of what it may still
	// hold, a tenant within its share kept, then any, least recently used first. Every tenant weighs
	// 1 until set, such as to the memory its sandboxes are given.
	SetMemoryWeight(context.Context, *SetMemoryWeightRequest) (*SetMemoryWeightResponse, error)
	// Sets the room a tenant's VM processes' connections share (TASK-149.35): what of their
	// requests the daemon holds at once, their data in the stream and what it queued for them, which
	// each connection's own room bounds too. A request past it waits for room. By default 8 GiB a
	// tenant, which a tenant's VMs reach only writing large extents through the stream, back to back.
	SetTenantRoom(context.Context, *SetTenantRoomRequest) (*SetTenantRoomResponse, error)
	// Evicts clean pages, of extents asked for longest ago first, until the memory a tenant, or the
	// daemon, holds is at most a target, or none is left to evict: as the host's memory runs low.
	ShrinkMemory(context.Context, *ShrinkMemoryRequest) (*ShrinkMemoryResponse, error)
	// Limits the host's disk a tenant takes, or every tenant together (TASK-94): what is in its
	// directory, its stores' files, each file once, the host's part cache no tenant's. Past a
	// tenant's limit, its VMs' writes, and the stores they make, fail with ENOSPC, which the VM's
	// guest sees as a full disk, and the daemon and other tenants' writes go on; past the total, any
	// tenant's do. Commits go on, and what leaves the disk (uploaded, freed, a store deleted) lets
	// the tenant write again. A limit under what is taken holds as data leaves.
	SetDiskLimit(context.Context, *SetDiskLimitRequest) (*SetDiskLimitResponse, error)
	// What the host's disk a tenant takes, or every tenant together, counted now, and its limit.
	GetTenantDisk(context.Context, *GetTenantDiskRequest) (*GetTenantDiskResponse, error)
	// Has the hosts sharing the daemon's bucket be these (TASK-406.2): its stores' fetchers ask the
	// host that owns a part, by rendezvous hashing of the object's key and the part's number over
	// them, for it before the bucket, and it serves them the parts it owns. Those it had are kept,
	// down or not. FAILED_PRECONDITION (ENOTSUP) if the daemon serves no peers (it was started
	// without FXDAX_EXTENTD_PEER_ADDRESS); INVALID_ARGUMENT (EINVAL) for an address that is no
	// HOST:PORT.
	SetPeers(context.Context, *SetPeersRequest) (*SetPeersResponse, error)
	mustEmbedUnimplementedExtentdServer()
}

// UnimplementedExtentdServer must be embedded to have
// forward compatible implementations.
//
// NOTE: this should be embedded by value instead of pointer to avoid a nil
// pointer dereference when methods are called.
type UnimplementedExtentdServer struct{}

func (UnimplementedExtentdServer) CreateStore(context.Context, *CreateStoreRequest) (*CreateStoreResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method CreateStore not implemented")
}
func (UnimplementedExtentdServer) ImportImage(context.Context, *ImportImageRequest) (*ImportImageResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method ImportImage not implemented")
}
func (UnimplementedExtentdServer) ForkStore(context.Context, *ForkStoreRequest) (*ForkStoreResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method ForkStore not implemented")
}
func (UnimplementedExtentdServer) DeleteStore(context.Context, *DeleteStoreRequest) (*DeleteStoreResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method DeleteStore not implemented")
}
func (UnimplementedExtentdServer) DropStore(context.Context, *DropStoreRequest) (*DropStoreResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method DropStore not implemented")
}
func (UnimplementedExtentdServer) ResizeStore(context.Context, *ResizeStoreRequest) (*ResizeStoreResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method ResizeStore not implemented")
}
func (UnimplementedExtentdServer) PlaceStore(context.Context, *PlaceStoreRequest) (*PlaceStoreResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method PlaceStore not implemented")
}
func (UnimplementedExtentdServer) MakePublic(context.Context, *MakePublicRequest) (*MakePublicResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method MakePublic not implemented")
}
func (UnimplementedExtentdServer) SharePoint(context.Context, *SharePointRequest) (*SharePointResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method SharePoint not implemented")
}
func (UnimplementedExtentdServer) AttachBase(context.Context, *AttachBaseRequest) (*AttachBaseResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method AttachBase not implemented")
}
func (UnimplementedExtentdServer) SyncStore(context.Context, *SyncStoreRequest) (*SyncStoreResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method SyncStore not implemented")
}
func (UnimplementedExtentdServer) StatStore(context.Context, *StatStoreRequest) (*StatStoreResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method StatStore not implemented")
}
func (UnimplementedExtentdServer) ListStores(context.Context, *ListStoresRequest) (*ListStoresResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method ListStores not implemented")
}
func (UnimplementedExtentdServer) GetStats(context.Context, *GetStatsRequest) (*GetStatsResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method GetStats not implemented")
}
func (UnimplementedExtentdServer) GetSnapshot(context.Context, *GetSnapshotRequest) (*GetSnapshotResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method GetSnapshot not implemented")
}
func (UnimplementedExtentdServer) Grant(context.Context, *GrantRequest) (*GrantResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method Grant not implemented")
}
func (UnimplementedExtentdServer) Revoke(context.Context, *RevokeRequest) (*RevokeResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method Revoke not implemented")
}
func (UnimplementedExtentdServer) SetMemoryLimit(context.Context, *SetMemoryLimitRequest) (*SetMemoryLimitResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method SetMemoryLimit not implemented")
}
func (UnimplementedExtentdServer) SetMemoryWeight(context.Context, *SetMemoryWeightRequest) (*SetMemoryWeightResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method SetMemoryWeight not implemented")
}
func (UnimplementedExtentdServer) SetTenantRoom(context.Context, *SetTenantRoomRequest) (*SetTenantRoomResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method SetTenantRoom not implemented")
}
func (UnimplementedExtentdServer) ShrinkMemory(context.Context, *ShrinkMemoryRequest) (*ShrinkMemoryResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method ShrinkMemory not implemented")
}
func (UnimplementedExtentdServer) SetDiskLimit(context.Context, *SetDiskLimitRequest) (*SetDiskLimitResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method SetDiskLimit not implemented")
}
func (UnimplementedExtentdServer) GetTenantDisk(context.Context, *GetTenantDiskRequest) (*GetTenantDiskResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method GetTenantDisk not implemented")
}
func (UnimplementedExtentdServer) SetPeers(context.Context, *SetPeersRequest) (*SetPeersResponse, error) {
	return nil, status.Error(codes.Unimplemented, "method SetPeers not implemented")
}
func (UnimplementedExtentdServer) mustEmbedUnimplementedExtentdServer() {}
func (UnimplementedExtentdServer) testEmbeddedByValue()                 {}

// UnsafeExtentdServer may be embedded to opt out of forward compatibility for this service.
// Use of this interface is not recommended, as added methods to ExtentdServer will
// result in compilation errors.
type UnsafeExtentdServer interface {
	mustEmbedUnimplementedExtentdServer()
}

func RegisterExtentdServer(s grpc.ServiceRegistrar, srv ExtentdServer) {
	// If the following call panics, it indicates UnimplementedExtentdServer was
	// embedded by pointer and is nil.  This will cause panics if an
	// unimplemented method is ever invoked, so we test this at initialization
	// time to prevent it from happening at runtime later due to I/O.
	if t, ok := srv.(interface{ testEmbeddedByValue() }); ok {
		t.testEmbeddedByValue()
	}
	s.RegisterService(&Extentd_ServiceDesc, srv)
}

func _Extentd_CreateStore_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(CreateStoreRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).CreateStore(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_CreateStore_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).CreateStore(ctx, req.(*CreateStoreRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_ImportImage_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(ImportImageRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).ImportImage(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_ImportImage_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).ImportImage(ctx, req.(*ImportImageRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_ForkStore_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(ForkStoreRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).ForkStore(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_ForkStore_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).ForkStore(ctx, req.(*ForkStoreRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_DeleteStore_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(DeleteStoreRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).DeleteStore(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_DeleteStore_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).DeleteStore(ctx, req.(*DeleteStoreRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_DropStore_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(DropStoreRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).DropStore(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_DropStore_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).DropStore(ctx, req.(*DropStoreRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_ResizeStore_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(ResizeStoreRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).ResizeStore(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_ResizeStore_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).ResizeStore(ctx, req.(*ResizeStoreRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_PlaceStore_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(PlaceStoreRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).PlaceStore(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_PlaceStore_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).PlaceStore(ctx, req.(*PlaceStoreRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_MakePublic_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(MakePublicRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).MakePublic(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_MakePublic_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).MakePublic(ctx, req.(*MakePublicRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_SharePoint_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(SharePointRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).SharePoint(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_SharePoint_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).SharePoint(ctx, req.(*SharePointRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_AttachBase_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(AttachBaseRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).AttachBase(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_AttachBase_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).AttachBase(ctx, req.(*AttachBaseRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_SyncStore_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(SyncStoreRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).SyncStore(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_SyncStore_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).SyncStore(ctx, req.(*SyncStoreRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_StatStore_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(StatStoreRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).StatStore(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_StatStore_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).StatStore(ctx, req.(*StatStoreRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_ListStores_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(ListStoresRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).ListStores(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_ListStores_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).ListStores(ctx, req.(*ListStoresRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_GetStats_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(GetStatsRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).GetStats(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_GetStats_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).GetStats(ctx, req.(*GetStatsRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_GetSnapshot_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(GetSnapshotRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).GetSnapshot(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_GetSnapshot_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).GetSnapshot(ctx, req.(*GetSnapshotRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_Grant_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(GrantRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).Grant(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_Grant_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).Grant(ctx, req.(*GrantRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_Revoke_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(RevokeRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).Revoke(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_Revoke_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).Revoke(ctx, req.(*RevokeRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_SetMemoryLimit_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(SetMemoryLimitRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).SetMemoryLimit(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_SetMemoryLimit_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).SetMemoryLimit(ctx, req.(*SetMemoryLimitRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_SetMemoryWeight_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(SetMemoryWeightRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).SetMemoryWeight(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_SetMemoryWeight_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).SetMemoryWeight(ctx, req.(*SetMemoryWeightRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_SetTenantRoom_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(SetTenantRoomRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).SetTenantRoom(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_SetTenantRoom_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).SetTenantRoom(ctx, req.(*SetTenantRoomRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_ShrinkMemory_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(ShrinkMemoryRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).ShrinkMemory(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_ShrinkMemory_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).ShrinkMemory(ctx, req.(*ShrinkMemoryRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_SetDiskLimit_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(SetDiskLimitRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).SetDiskLimit(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_SetDiskLimit_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).SetDiskLimit(ctx, req.(*SetDiskLimitRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_GetTenantDisk_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(GetTenantDiskRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).GetTenantDisk(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_GetTenantDisk_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).GetTenantDisk(ctx, req.(*GetTenantDiskRequest))
	}
	return interceptor(ctx, in, info, handler)
}

func _Extentd_SetPeers_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(SetPeersRequest)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(ExtentdServer).SetPeers(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Extentd_SetPeers_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(ExtentdServer).SetPeers(ctx, req.(*SetPeersRequest))
	}
	return interceptor(ctx, in, info, handler)
}

// Extentd_ServiceDesc is the grpc.ServiceDesc for Extentd service.
// It's only intended for direct use with grpc.RegisterService,
// and not to be introspected or modified (even as a copy)
var Extentd_ServiceDesc = grpc.ServiceDesc{
	ServiceName: "fxdax.extentd.v1.Extentd",
	HandlerType: (*ExtentdServer)(nil),
	Methods: []grpc.MethodDesc{
		{
			MethodName: "CreateStore",
			Handler:    _Extentd_CreateStore_Handler,
		},
		{
			MethodName: "ImportImage",
			Handler:    _Extentd_ImportImage_Handler,
		},
		{
			MethodName: "ForkStore",
			Handler:    _Extentd_ForkStore_Handler,
		},
		{
			MethodName: "DeleteStore",
			Handler:    _Extentd_DeleteStore_Handler,
		},
		{
			MethodName: "DropStore",
			Handler:    _Extentd_DropStore_Handler,
		},
		{
			MethodName: "ResizeStore",
			Handler:    _Extentd_ResizeStore_Handler,
		},
		{
			MethodName: "PlaceStore",
			Handler:    _Extentd_PlaceStore_Handler,
		},
		{
			MethodName: "MakePublic",
			Handler:    _Extentd_MakePublic_Handler,
		},
		{
			MethodName: "SharePoint",
			Handler:    _Extentd_SharePoint_Handler,
		},
		{
			MethodName: "AttachBase",
			Handler:    _Extentd_AttachBase_Handler,
		},
		{
			MethodName: "SyncStore",
			Handler:    _Extentd_SyncStore_Handler,
		},
		{
			MethodName: "StatStore",
			Handler:    _Extentd_StatStore_Handler,
		},
		{
			MethodName: "ListStores",
			Handler:    _Extentd_ListStores_Handler,
		},
		{
			MethodName: "GetStats",
			Handler:    _Extentd_GetStats_Handler,
		},
		{
			MethodName: "GetSnapshot",
			Handler:    _Extentd_GetSnapshot_Handler,
		},
		{
			MethodName: "Grant",
			Handler:    _Extentd_Grant_Handler,
		},
		{
			MethodName: "Revoke",
			Handler:    _Extentd_Revoke_Handler,
		},
		{
			MethodName: "SetMemoryLimit",
			Handler:    _Extentd_SetMemoryLimit_Handler,
		},
		{
			MethodName: "SetMemoryWeight",
			Handler:    _Extentd_SetMemoryWeight_Handler,
		},
		{
			MethodName: "SetTenantRoom",
			Handler:    _Extentd_SetTenantRoom_Handler,
		},
		{
			MethodName: "ShrinkMemory",
			Handler:    _Extentd_ShrinkMemory_Handler,
		},
		{
			MethodName: "SetDiskLimit",
			Handler:    _Extentd_SetDiskLimit_Handler,
		},
		{
			MethodName: "GetTenantDisk",
			Handler:    _Extentd_GetTenantDisk_Handler,
		},
		{
			MethodName: "SetPeers",
			Handler:    _Extentd_SetPeers_Handler,
		},
	},
	Streams:  []grpc.StreamDesc{},
	Metadata: "fxdax/extentd/v1/extentd.proto",
}
