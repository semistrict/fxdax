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

// Code generated by protoc-gen-go. DO NOT EDIT.
// versions:
// 	protoc-gen-go v1.36.12
// 	protoc        (unknown)
// source: fxdax/extentd/v1/extentd.proto

package extentdv1

import (
	protoreflect "google.golang.org/protobuf/reflect/protoreflect"
	protoimpl "google.golang.org/protobuf/runtime/protoimpl"
	reflect "reflect"
	sync "sync"
	unsafe "unsafe"
)

const (
	// Verify that this generated code is sufficiently up-to-date.
	_ = protoimpl.EnforceVersion(20 - protoimpl.MinVersion)
	// Verify that runtime/protoimpl is sufficiently up-to-date.
	_ = protoimpl.EnforceVersion(protoimpl.MaxVersion - 20)
)

// What a volume of a cell holds.
type VolumeKind int32

const (
	// An Fxfs filesystem: a VM's disk, or a host's mount's.
	VolumeKind_VOLUME_KIND_FXFS VolumeKind = 0
	// A database of sqlite_extents, which its VFS alone opens.
	VolumeKind_VOLUME_KIND_SQLITE VolumeKind = 1
)

// Enum value maps for VolumeKind.
var (
	VolumeKind_name = map[int32]string{
		0: "VOLUME_KIND_FXFS",
		1: "VOLUME_KIND_SQLITE",
	}
	VolumeKind_value = map[string]int32{
		"VOLUME_KIND_FXFS":   0,
		"VOLUME_KIND_SQLITE": 1,
	}
)

func (x VolumeKind) Enum() *VolumeKind {
	p := new(VolumeKind)
	*p = x
	return p
}

func (x VolumeKind) String() string {
	return protoimpl.X.EnumStringOf(x.Descriptor(), protoreflect.EnumNumber(x))
}

func (VolumeKind) Descriptor() protoreflect.EnumDescriptor {
	return file_fxdax_extentd_v1_extentd_proto_enumTypes[0].Descriptor()
}

func (VolumeKind) Type() protoreflect.EnumType {
	return &file_fxdax_extentd_v1_extentd_proto_enumTypes[0]
}

func (x VolumeKind) Number() protoreflect.EnumNumber {
	return protoreflect.EnumNumber(x)
}

// Deprecated: Use VolumeKind.Descriptor instead.
func (VolumeKind) EnumDescriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{0}
}

// Makes an empty store.
type CreateStoreRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Where to make it: a directory that must not exist (ALREADY_EXISTS).
	Path string `protobuf:"bytes,1,opt,name=path,proto3" json:"path,omitempty"`
	// The most bytes it holds.
	Capacity uint64 `protobuf:"varint,2,opt,name=capacity,proto3" json:"capacity,omitempty"`
	// The branch kept in the daemon's bucket to take, of `cell` (NOT_FOUND, ENOENT, if it has none;
	// INVALID_ARGUMENT, EINVAL, of a cell of 0); or 0 for a new branch, its ID the bucket's to give
	// (TASK-364), which the response says.
	Branch uint64 `protobuf:"varint,3,opt,name=branch,proto3" json:"branch,omitempty"`
	// The fencing token to place it at (PlaceStore), if any: of a placement no earlier than the
	// last writer's of the branch to take, or the take fails (FAILED_PRECONDITION, ESTALE). A point
	// has no writer, and needs none.
	Token *uint64 `protobuf:"varint,4,opt,name=token,proto3,oneof" json:"token,omitempty"`
	// The tenant whose cell the branch is a volume of. Of a new cell in a granted tenant's
	// directory, that tenant's ID (Grant's `id`), or 0 for it; another is refused
	// (PERMISSION_DENIED, EPERM). A point of another tenant's there is the VM manager's to give: the
	// tenant's VM processes restore from it, each restore a cell of their tenant's, if the point is
	// shared (SharePoint); otherwise each restore fails (PERMISSION_DENIED, EACCES).
	Tenant uint64 `protobuf:"varint,5,opt,name=tenant,proto3" json:"tenant,omitempty"`
	// The cell, or 0 for a new one: the store is a new volume of it, named as `path`'s last name
	// is, unless it takes a branch it has.
	Cell uint64 `protobuf:"varint,6,opt,name=cell,proto3" json:"cell,omitempty"`
	// What the volume holds, which says what opens it.
	Kind          VolumeKind `protobuf:"varint,7,opt,name=kind,proto3,enum=fxdax.extentd.v1.VolumeKind" json:"kind,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *CreateStoreRequest) Reset() {
	*x = CreateStoreRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[0]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *CreateStoreRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*CreateStoreRequest) ProtoMessage() {}

func (x *CreateStoreRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[0]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use CreateStoreRequest.ProtoReflect.Descriptor instead.
func (*CreateStoreRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{0}
}

func (x *CreateStoreRequest) GetPath() string {
	if x != nil {
		return x.Path
	}
	return ""
}

func (x *CreateStoreRequest) GetCapacity() uint64 {
	if x != nil {
		return x.Capacity
	}
	return 0
}

func (x *CreateStoreRequest) GetBranch() uint64 {
	if x != nil {
		return x.Branch
	}
	return 0
}

func (x *CreateStoreRequest) GetToken() uint64 {
	if x != nil && x.Token != nil {
		return *x.Token
	}
	return 0
}

func (x *CreateStoreRequest) GetTenant() uint64 {
	if x != nil {
		return x.Tenant
	}
	return 0
}

func (x *CreateStoreRequest) GetCell() uint64 {
	if x != nil {
		return x.Cell
	}
	return 0
}

func (x *CreateStoreRequest) GetKind() VolumeKind {
	if x != nil {
		return x.Kind
	}
	return VolumeKind_VOLUME_KIND_FXFS
}

// The store made.
type CreateStoreResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Its branch: the one taken, or the new one.
	Branch uint64 `protobuf:"varint,1,opt,name=branch,proto3" json:"branch,omitempty"`
	// Its tenant.
	Tenant uint64 `protobuf:"varint,2,opt,name=tenant,proto3" json:"tenant,omitempty"`
	// Its cell: the one named, or the new one.
	Cell          uint64 `protobuf:"varint,3,opt,name=cell,proto3" json:"cell,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *CreateStoreResponse) Reset() {
	*x = CreateStoreResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[1]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *CreateStoreResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*CreateStoreResponse) ProtoMessage() {}

func (x *CreateStoreResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[1]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use CreateStoreResponse.ProtoReflect.Descriptor instead.
func (*CreateStoreResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{1}
}

func (x *CreateStoreResponse) GetBranch() uint64 {
	if x != nil {
		return x.Branch
	}
	return 0
}

func (x *CreateStoreResponse) GetTenant() uint64 {
	if x != nil {
		return x.Tenant
	}
	return 0
}

func (x *CreateStoreResponse) GetCell() uint64 {
	if x != nil {
		return x.Cell
	}
	return 0
}

// Makes a store of a container image.
type ImportImageRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Where to make it: a directory that must not exist (ALREADY_EXISTS).
	Path string `protobuf:"bytes,1,opt,name=path,proto3" json:"path,omitempty"`
	// The most bytes it holds.
	Capacity uint64 `protobuf:"varint,2,opt,name=capacity,proto3" json:"capacity,omitempty"`
	// The image, on the daemon's host: an OCI image layout (as `skopeo copy` makes, or
	// `docker save`'s tarball extracted), or a tar archive of one layer, gzipped or not (as
	// `docker export` makes).
	Image string `protobuf:"bytes,3,opt,name=image,proto3" json:"image,omitempty"`
	// The image in the layout to import, by its reference; empty for its only image.
	Reference string `protobuf:"bytes,4,opt,name=reference,proto3" json:"reference,omitempty"`
	// The tenant of the new cell the store is the one volume of. In a granted tenant's directory,
	// that tenant's ID, or 0 for it; another is refused (PERMISSION_DENIED, EPERM).
	Tenant        uint64 `protobuf:"varint,5,opt,name=tenant,proto3" json:"tenant,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ImportImageRequest) Reset() {
	*x = ImportImageRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[2]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ImportImageRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ImportImageRequest) ProtoMessage() {}

func (x *ImportImageRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[2]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ImportImageRequest.ProtoReflect.Descriptor instead.
func (*ImportImageRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{2}
}

func (x *ImportImageRequest) GetPath() string {
	if x != nil {
		return x.Path
	}
	return ""
}

func (x *ImportImageRequest) GetCapacity() uint64 {
	if x != nil {
		return x.Capacity
	}
	return 0
}

func (x *ImportImageRequest) GetImage() string {
	if x != nil {
		return x.Image
	}
	return ""
}

func (x *ImportImageRequest) GetReference() string {
	if x != nil {
		return x.Reference
	}
	return ""
}

func (x *ImportImageRequest) GetTenant() uint64 {
	if x != nil {
		return x.Tenant
	}
	return 0
}

// The store made.
type ImportImageResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Its branch, a new one.
	Branch uint64 `protobuf:"varint,1,opt,name=branch,proto3" json:"branch,omitempty"`
	// Its tenant.
	Tenant uint64 `protobuf:"varint,2,opt,name=tenant,proto3" json:"tenant,omitempty"`
	// Its cell, a new one, of which it is the one volume.
	Cell          uint64 `protobuf:"varint,3,opt,name=cell,proto3" json:"cell,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ImportImageResponse) Reset() {
	*x = ImportImageResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[3]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ImportImageResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ImportImageResponse) ProtoMessage() {}

func (x *ImportImageResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[3]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ImportImageResponse.ProtoReflect.Descriptor instead.
func (*ImportImageResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{3}
}

func (x *ImportImageResponse) GetBranch() uint64 {
	if x != nil {
		return x.Branch
	}
	return 0
}

func (x *ImportImageResponse) GetTenant() uint64 {
	if x != nil {
		return x.Tenant
	}
	return 0
}

func (x *ImportImageResponse) GetCell() uint64 {
	if x != nil {
		return x.Cell
	}
	return 0
}

// Forks a store.
type ForkStoreRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The store to fork.
	From string `protobuf:"bytes,1,opt,name=from,proto3" json:"from,omitempty"`
	// Where the fork goes: a directory that must not exist (ALREADY_EXISTS), made a store of its
	// own, holding the fork's branch alone. The fork is made in the daemon's bucket (backlog doc-7,
	// Forks): `from`'s commit naming the fork, once acknowledged there, gives the fork's branch its
	// record, inheriting `from`'s extents rather than copies; only then is `to` made, taking the
	// branch's state from the bucket. A fork that fails deletes `to`, and its branch in the bucket
	// if its commit was made; its host crashing part way, `to` is left unmade, which DeleteStore of
	// `to` deletes, and the fork stands or never was, as the bucket has it.
	To string `protobuf:"bytes,2,opt,name=to,proto3" json:"to,omitempty"`
	// Whether the fork is a point (TASK-158): a name of `from`'s state as it is, which any number of
	// hosts read and fork at once, and none writes, as a snapshot's is. Forks of it on another host:
	// CreateStore of its branch there, then ForkStore from that copy, as many and on as many hosts
	// at once as wanted. `from` must have nothing open (FAILED_PRECONDITION, EBUSY), as a fork of a
	// store a VM has open does, and be no point itself (INVALID_ARGUMENT): fork that instead.
	Point bool `protobuf:"varint,4,opt,name=point,proto3" json:"point,omitempty"`
	// Whether the point is shared once made, as SharePoint shares it (TASK-395.7): only with
	// `point` (INVALID_ARGUMENT otherwise). If sharing it fails, the point stands, not shared, and
	// the call fails: SharePoint shares it then.
	Shared        bool `protobuf:"varint,5,opt,name=shared,proto3" json:"shared,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ForkStoreRequest) Reset() {
	*x = ForkStoreRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[4]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ForkStoreRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ForkStoreRequest) ProtoMessage() {}

func (x *ForkStoreRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[4]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ForkStoreRequest.ProtoReflect.Descriptor instead.
func (*ForkStoreRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{4}
}

func (x *ForkStoreRequest) GetFrom() string {
	if x != nil {
		return x.From
	}
	return ""
}

func (x *ForkStoreRequest) GetTo() string {
	if x != nil {
		return x.To
	}
	return ""
}

func (x *ForkStoreRequest) GetPoint() bool {
	if x != nil {
		return x.Point
	}
	return false
}

func (x *ForkStoreRequest) GetShared() bool {
	if x != nil {
		return x.Shared
	}
	return false
}

// The fork made.
type ForkStoreResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Its branch.
	Branch uint64 `protobuf:"varint,1,opt,name=branch,proto3" json:"branch,omitempty"`
	// Its tenant: `from`'s.
	Tenant uint64 `protobuf:"varint,2,opt,name=tenant,proto3" json:"tenant,omitempty"`
	// Its cell, a new one, of which it is the one volume.
	Cell          uint64 `protobuf:"varint,3,opt,name=cell,proto3" json:"cell,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ForkStoreResponse) Reset() {
	*x = ForkStoreResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[5]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ForkStoreResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ForkStoreResponse) ProtoMessage() {}

func (x *ForkStoreResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[5]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ForkStoreResponse.ProtoReflect.Descriptor instead.
func (*ForkStoreResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{5}
}

func (x *ForkStoreResponse) GetBranch() uint64 {
	if x != nil {
		return x.Branch
	}
	return 0
}

func (x *ForkStoreResponse) GetTenant() uint64 {
	if x != nil {
		return x.Tenant
	}
	return 0
}

func (x *ForkStoreResponse) GetCell() uint64 {
	if x != nil {
		return x.Cell
	}
	return 0
}

// Deletes a store.
type DeleteStoreRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The store, or the fork, to delete.
	Path          string `protobuf:"bytes,1,opt,name=path,proto3" json:"path,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *DeleteStoreRequest) Reset() {
	*x = DeleteStoreRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[6]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *DeleteStoreRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*DeleteStoreRequest) ProtoMessage() {}

func (x *DeleteStoreRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[6]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use DeleteStoreRequest.ProtoReflect.Descriptor instead.
func (*DeleteStoreRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{6}
}

func (x *DeleteStoreRequest) GetPath() string {
	if x != nil {
		return x.Path
	}
	return ""
}

// The store deleted.
type DeleteStoreResponse struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *DeleteStoreResponse) Reset() {
	*x = DeleteStoreResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[7]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *DeleteStoreResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*DeleteStoreResponse) ProtoMessage() {}

func (x *DeleteStoreResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[7]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use DeleteStoreResponse.ProtoReflect.Descriptor instead.
func (*DeleteStoreResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{7}
}

// Deletes this host's copy of a store.
type DropStoreRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The store, or the fork, to delete here.
	Path          string `protobuf:"bytes,1,opt,name=path,proto3" json:"path,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *DropStoreRequest) Reset() {
	*x = DropStoreRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[8]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *DropStoreRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*DropStoreRequest) ProtoMessage() {}

func (x *DropStoreRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[8]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use DropStoreRequest.ProtoReflect.Descriptor instead.
func (*DropStoreRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{8}
}

func (x *DropStoreRequest) GetPath() string {
	if x != nil {
		return x.Path
	}
	return ""
}

// This host's copy deleted.
type DropStoreResponse struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *DropStoreResponse) Reset() {
	*x = DropStoreResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[9]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *DropStoreResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*DropStoreResponse) ProtoMessage() {}

func (x *DropStoreResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[9]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use DropStoreResponse.ProtoReflect.Descriptor instead.
func (*DropStoreResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{9}
}

// Sets a store's capacity.
type ResizeStoreRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The store to resize.
	Path string `protobuf:"bytes,1,opt,name=path,proto3" json:"path,omitempty"`
	// Its new capacity, in bytes.
	Capacity      uint64 `protobuf:"varint,2,opt,name=capacity,proto3" json:"capacity,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ResizeStoreRequest) Reset() {
	*x = ResizeStoreRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[10]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ResizeStoreRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ResizeStoreRequest) ProtoMessage() {}

func (x *ResizeStoreRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[10]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ResizeStoreRequest.ProtoReflect.Descriptor instead.
func (*ResizeStoreRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{10}
}

func (x *ResizeStoreRequest) GetPath() string {
	if x != nil {
		return x.Path
	}
	return ""
}

func (x *ResizeStoreRequest) GetCapacity() uint64 {
	if x != nil {
		return x.Capacity
	}
	return 0
}

// The store resized.
type ResizeStoreResponse struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ResizeStoreResponse) Reset() {
	*x = ResizeStoreResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[11]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ResizeStoreResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ResizeStoreResponse) ProtoMessage() {}

func (x *ResizeStoreResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[11]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ResizeStoreResponse.ProtoReflect.Descriptor instead.
func (*ResizeStoreResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{11}
}

// Places a store.
type PlaceStoreRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The store, or the fork, to place.
	Path string `protobuf:"bytes,1,opt,name=path,proto3" json:"path,omitempty"`
	// The placement's fencing token, its boot ID: greater than every earlier placement's.
	Token         uint64 `protobuf:"varint,2,opt,name=token,proto3" json:"token,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *PlaceStoreRequest) Reset() {
	*x = PlaceStoreRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[12]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *PlaceStoreRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*PlaceStoreRequest) ProtoMessage() {}

func (x *PlaceStoreRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[12]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use PlaceStoreRequest.ProtoReflect.Descriptor instead.
func (*PlaceStoreRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{12}
}

func (x *PlaceStoreRequest) GetPath() string {
	if x != nil {
		return x.Path
	}
	return ""
}

func (x *PlaceStoreRequest) GetToken() uint64 {
	if x != nil {
		return x.Token
	}
	return 0
}

// The store placed.
type PlaceStoreResponse struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *PlaceStoreResponse) Reset() {
	*x = PlaceStoreResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[13]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *PlaceStoreResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*PlaceStoreResponse) ProtoMessage() {}

func (x *PlaceStoreResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[13]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use PlaceStoreResponse.ProtoReflect.Descriptor instead.
func (*PlaceStoreResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{13}
}

// Shares a point, or no longer.
type SharePointRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// A store of the point: the one ForkStore made, or a copy CreateStore made, on this host.
	Path string `protobuf:"bytes,1,opt,name=path,proto3" json:"path,omitempty"`
	// Whether other tenants' forks of it are made from now on.
	Shared        bool `protobuf:"varint,2,opt,name=shared,proto3" json:"shared,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SharePointRequest) Reset() {
	*x = SharePointRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[14]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SharePointRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SharePointRequest) ProtoMessage() {}

func (x *SharePointRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[14]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SharePointRequest.ProtoReflect.Descriptor instead.
func (*SharePointRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{14}
}

func (x *SharePointRequest) GetPath() string {
	if x != nil {
		return x.Path
	}
	return ""
}

func (x *SharePointRequest) GetShared() bool {
	if x != nil {
		return x.Shared
	}
	return false
}

// The point shared, or no longer.
type SharePointResponse struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SharePointResponse) Reset() {
	*x = SharePointResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[15]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SharePointResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SharePointResponse) ProtoMessage() {}

func (x *SharePointResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[15]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SharePointResponse.ProtoReflect.Descriptor instead.
func (*SharePointResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{15}
}

// Makes a store a public base.
type MakePublicRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The store, which no VM may have open.
	Path string `protobuf:"bytes,1,opt,name=path,proto3" json:"path,omitempty"`
	// Its branch alone, not those it inherited: a fork pinning a generation of a store that goes on
	// being written, to attach to stores (AttachBase), not to fork.
	Generation    bool `protobuf:"varint,2,opt,name=generation,proto3" json:"generation,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *MakePublicRequest) Reset() {
	*x = MakePublicRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[16]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *MakePublicRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*MakePublicRequest) ProtoMessage() {}

func (x *MakePublicRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[16]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use MakePublicRequest.ProtoReflect.Descriptor instead.
func (*MakePublicRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{16}
}

func (x *MakePublicRequest) GetPath() string {
	if x != nil {
		return x.Path
	}
	return ""
}

func (x *MakePublicRequest) GetGeneration() bool {
	if x != nil {
		return x.Generation
	}
	return false
}

// The store made public.
type MakePublicResponse struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *MakePublicResponse) Reset() {
	*x = MakePublicResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[17]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *MakePublicResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*MakePublicResponse) ProtoMessage() {}

func (x *MakePublicResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[17]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use MakePublicResponse.ProtoReflect.Descriptor instead.
func (*MakePublicResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{17}
}

// Attaches a fork to a store.
type AttachBaseRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The store to attach it to.
	Store string `protobuf:"bytes,1,opt,name=store,proto3" json:"store,omitempty"`
	// The fork.
	Base          string `protobuf:"bytes,2,opt,name=base,proto3" json:"base,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *AttachBaseRequest) Reset() {
	*x = AttachBaseRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[18]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *AttachBaseRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*AttachBaseRequest) ProtoMessage() {}

func (x *AttachBaseRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[18]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use AttachBaseRequest.ProtoReflect.Descriptor instead.
func (*AttachBaseRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{18}
}

func (x *AttachBaseRequest) GetStore() string {
	if x != nil {
		return x.Store
	}
	return ""
}

func (x *AttachBaseRequest) GetBase() string {
	if x != nil {
		return x.Base
	}
	return ""
}

// Attached, durably.
type AttachBaseResponse struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *AttachBaseResponse) Reset() {
	*x = AttachBaseResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[19]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *AttachBaseResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*AttachBaseResponse) ProtoMessage() {}

func (x *AttachBaseResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[19]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use AttachBaseResponse.ProtoReflect.Descriptor instead.
func (*AttachBaseResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{19}
}

// Waits for a store's commits to be in object storage.
type SyncStoreRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The store to wait for.
	Path          string `protobuf:"bytes,1,opt,name=path,proto3" json:"path,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SyncStoreRequest) Reset() {
	*x = SyncStoreRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[20]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SyncStoreRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SyncStoreRequest) ProtoMessage() {}

func (x *SyncStoreRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[20]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SyncStoreRequest.ProtoReflect.Descriptor instead.
func (*SyncStoreRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{20}
}

func (x *SyncStoreRequest) GetPath() string {
	if x != nil {
		return x.Path
	}
	return ""
}

// The store's commits, as of the call, are in object storage.
type SyncStoreResponse struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SyncStoreResponse) Reset() {
	*x = SyncStoreResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[21]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SyncStoreResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SyncStoreResponse) ProtoMessage() {}

func (x *SyncStoreResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[21]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SyncStoreResponse.ProtoReflect.Descriptor instead.
func (*SyncStoreResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{21}
}

// Asks what a store is, and holds.
type StatStoreRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The store to look at.
	Path          string `protobuf:"bytes,1,opt,name=path,proto3" json:"path,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *StatStoreRequest) Reset() {
	*x = StatStoreRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[22]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *StatStoreRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*StatStoreRequest) ProtoMessage() {}

func (x *StatStoreRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[22]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use StatStoreRequest.ProtoReflect.Descriptor instead.
func (*StatStoreRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{22}
}

func (x *StatStoreRequest) GetPath() string {
	if x != nil {
		return x.Path
	}
	return ""
}

// What a store is, and holds.
type StatStoreResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The branch whose extents its guest mints.
	Branch uint64 `protobuf:"varint,1,opt,name=branch,proto3" json:"branch,omitempty"`
	// The most bytes it holds.
	Capacity uint64 `protobuf:"varint,2,opt,name=capacity,proto3" json:"capacity,omitempty"`
	// The bytes its extents hold.
	Used uint64 `protobuf:"varint,3,opt,name=used,proto3" json:"used,omitempty"`
	// The bytes left for new extents.
	Available uint64 `protobuf:"varint,4,opt,name=available,proto3" json:"available,omitempty"`
	// Whether it is a public base (MakePublic).
	Public bool `protobuf:"varint,5,opt,name=public,proto3" json:"public,omitempty"`
	// Whether a VM process has it open.
	Open bool `protobuf:"varint,6,opt,name=open,proto3" json:"open,omitempty"`
	// Of the bytes its extents hold (used), those of the extents its branch wrote: what billing it
	// alone charges it (TASK-49).
	Own uint64 `protobuf:"varint,7,opt,name=own,proto3" json:"own,omitempty"`
	// Those of the extents it inherited from the branches it was forked from, which may still share
	// them. Views of followers may keep more, freed: used is at least own and inherited.
	Inherited uint64 `protobuf:"varint,8,opt,name=inherited,proto3" json:"inherited,omitempty"`
	// The epoch of its latest commit: what a follower of it advances to.
	Committed uint64 `protobuf:"varint,9,opt,name=committed,proto3" json:"committed,omitempty"`
	// Whether its commits upload to the daemon's bucket now: while a VM process has it open, as
	// writer. If not, the fields below are zero (TASK-50.5).
	Uploading bool `protobuf:"varint,10,opt,name=uploading,proto3" json:"uploading,omitempty"`
	// The fencing token of the placement whose writer took its branch's lease: this host's.
	LeaseToken uint64 `protobuf:"varint,11,opt,name=lease_token,json=leaseToken,proto3" json:"lease_token,omitempty"`
	// The lease's epoch, one more than its last taking's.
	LeaseEpoch uint64 `protobuf:"varint,12,opt,name=lease_epoch,json=leaseEpoch,proto3" json:"lease_epoch,omitempty"`
	// When the writer took the lease, in milliseconds since the Unix epoch.
	LeaseTakenUnixMs uint64 `protobuf:"varint,13,opt,name=lease_taken_unix_ms,json=leaseTakenUnixMs,proto3" json:"lease_taken_unix_ms,omitempty"`
	// The epoch of its latest commit in the bucket.
	Uploaded uint64 `protobuf:"varint,14,opt,name=uploaded,proto3" json:"uploaded,omitempty"`
	// The bytes its commits not yet in the bucket wrote.
	PendingUploadBytes uint64 `protobuf:"varint,15,opt,name=pending_upload_bytes,json=pendingUploadBytes,proto3" json:"pending_upload_bytes,omitempty"`
	// How long the oldest commit not yet in the bucket has waited, in milliseconds, to within the
	// uploader's interval (a second): 0 if none waits.
	OldestPendingMs uint64 `protobuf:"varint,16,opt,name=oldest_pending_ms,json=oldestPendingMs,proto3" json:"oldest_pending_ms,omitempty"`
	// The tenant whose cell its branch is a volume of (TASK-364).
	Tenant uint64 `protobuf:"varint,17,opt,name=tenant,proto3" json:"tenant,omitempty"`
	// The cell its branch is a volume of.
	Cell          uint64 `protobuf:"varint,18,opt,name=cell,proto3" json:"cell,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *StatStoreResponse) Reset() {
	*x = StatStoreResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[23]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *StatStoreResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*StatStoreResponse) ProtoMessage() {}

func (x *StatStoreResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[23]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use StatStoreResponse.ProtoReflect.Descriptor instead.
func (*StatStoreResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{23}
}

func (x *StatStoreResponse) GetBranch() uint64 {
	if x != nil {
		return x.Branch
	}
	return 0
}

func (x *StatStoreResponse) GetCapacity() uint64 {
	if x != nil {
		return x.Capacity
	}
	return 0
}

func (x *StatStoreResponse) GetUsed() uint64 {
	if x != nil {
		return x.Used
	}
	return 0
}

func (x *StatStoreResponse) GetAvailable() uint64 {
	if x != nil {
		return x.Available
	}
	return 0
}

func (x *StatStoreResponse) GetPublic() bool {
	if x != nil {
		return x.Public
	}
	return false
}

func (x *StatStoreResponse) GetOpen() bool {
	if x != nil {
		return x.Open
	}
	return false
}

func (x *StatStoreResponse) GetOwn() uint64 {
	if x != nil {
		return x.Own
	}
	return 0
}

func (x *StatStoreResponse) GetInherited() uint64 {
	if x != nil {
		return x.Inherited
	}
	return 0
}

func (x *StatStoreResponse) GetCommitted() uint64 {
	if x != nil {
		return x.Committed
	}
	return 0
}

func (x *StatStoreResponse) GetUploading() bool {
	if x != nil {
		return x.Uploading
	}
	return false
}

func (x *StatStoreResponse) GetLeaseToken() uint64 {
	if x != nil {
		return x.LeaseToken
	}
	return 0
}

func (x *StatStoreResponse) GetLeaseEpoch() uint64 {
	if x != nil {
		return x.LeaseEpoch
	}
	return 0
}

func (x *StatStoreResponse) GetLeaseTakenUnixMs() uint64 {
	if x != nil {
		return x.LeaseTakenUnixMs
	}
	return 0
}

func (x *StatStoreResponse) GetUploaded() uint64 {
	if x != nil {
		return x.Uploaded
	}
	return 0
}

func (x *StatStoreResponse) GetPendingUploadBytes() uint64 {
	if x != nil {
		return x.PendingUploadBytes
	}
	return 0
}

func (x *StatStoreResponse) GetOldestPendingMs() uint64 {
	if x != nil {
		return x.OldestPendingMs
	}
	return 0
}

func (x *StatStoreResponse) GetTenant() uint64 {
	if x != nil {
		return x.Tenant
	}
	return 0
}

func (x *StatStoreResponse) GetCell() uint64 {
	if x != nil {
		return x.Cell
	}
	return 0
}

// Asks for the stores the daemon serves.
type ListStoresRequest struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ListStoresRequest) Reset() {
	*x = ListStoresRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[24]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ListStoresRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ListStoresRequest) ProtoMessage() {}

func (x *ListStoresRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[24]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ListStoresRequest.ProtoReflect.Descriptor instead.
func (*ListStoresRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{24}
}

// The stores the daemon serves, in no order.
type ListStoresResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Each store, and what it is and holds.
	Stores        []*ListedStore `protobuf:"bytes,1,rep,name=stores,proto3" json:"stores,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ListStoresResponse) Reset() {
	*x = ListStoresResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[25]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ListStoresResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ListStoresResponse) ProtoMessage() {}

func (x *ListStoresResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[25]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ListStoresResponse.ProtoReflect.Descriptor instead.
func (*ListStoresResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{25}
}

func (x *ListStoresResponse) GetStores() []*ListedStore {
	if x != nil {
		return x.Stores
	}
	return nil
}

// A store the daemon serves.
type ListedStore struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Its directory, as the daemon has it: without symbolic links.
	Path string `protobuf:"bytes,1,opt,name=path,proto3" json:"path,omitempty"`
	// What StatStore says of it.
	Stat          *StatStoreResponse `protobuf:"bytes,2,opt,name=stat,proto3" json:"stat,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ListedStore) Reset() {
	*x = ListedStore{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[26]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ListedStore) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ListedStore) ProtoMessage() {}

func (x *ListedStore) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[26]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ListedStore.ProtoReflect.Descriptor instead.
func (*ListedStore) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{26}
}

func (x *ListedStore) GetPath() string {
	if x != nil {
		return x.Path
	}
	return ""
}

func (x *ListedStore) GetStat() *StatStoreResponse {
	if x != nil {
		return x.Stat
	}
	return nil
}

// Asks what the daemon did.
type GetStatsRequest struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *GetStatsRequest) Reset() {
	*x = GetStatsRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[27]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *GetStatsRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*GetStatsRequest) ProtoMessage() {}

func (x *GetStatsRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[27]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use GetStatsRequest.ProtoReflect.Descriptor instead.
func (*GetStatsRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{27}
}

// What the daemon did since it started: counters, which only grow, so that a measurement is the
// difference of two.
type GetStatsResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// MAP requests of VMs' windows.
	Maps uint64 `protobuf:"varint,1,opt,name=maps,proto3" json:"maps,omitempty"`
	// The nanoseconds they took.
	MapNs uint64 `protobuf:"varint,2,opt,name=map_ns,json=mapNs,proto3" json:"map_ns,omitempty"`
	// Guests' faults on their windows resolved.
	Faults uint64 `protobuf:"varint,3,opt,name=faults,proto3" json:"faults,omitempty"`
	// The nanoseconds they took.
	FaultNs uint64 `protobuf:"varint,4,opt,name=fault_ns,json=faultNs,proto3" json:"fault_ns,omitempty"`
	// Reads of sealed extents' data into memory.
	PageIns uint64 `protobuf:"varint,5,opt,name=page_ins,json=pageIns,proto3" json:"page_ins,omitempty"`
	// The bytes they read from local stores.
	PageInBytes uint64 `protobuf:"varint,6,opt,name=page_in_bytes,json=pageInBytes,proto3" json:"page_in_bytes,omitempty"`
	// The bytes they fetched from object storage.
	FetchedBytes uint64 `protobuf:"varint,7,opt,name=fetched_bytes,json=fetchedBytes,proto3" json:"fetched_bytes,omitempty"`
	// READ requests.
	Reads uint64 `protobuf:"varint,8,opt,name=reads,proto3" json:"reads,omitempty"`
	// The bytes they read.
	ReadBytes uint64 `protobuf:"varint,9,opt,name=read_bytes,json=readBytes,proto3" json:"read_bytes,omitempty"`
	// Of those, the bytes read straight from local stores or object storage: of open extents, and
	// followers'. Sealed extents' are read through the extent cache, whose page-ins count them.
	ReadStoredBytes uint64 `protobuf:"varint,10,opt,name=read_stored_bytes,json=readStoredBytes,proto3" json:"read_stored_bytes,omitempty"`
	// Units of boot profiles read into memory ahead of forks.
	Prefetches uint64 `protobuf:"varint,11,opt,name=prefetches,proto3" json:"prefetches,omitempty"`
	// Parts of extents stored compressed, decompressed.
	Unpacks uint64 `protobuf:"varint,12,opt,name=unpacks,proto3" json:"unpacks,omitempty"`
	// What the daemon did with its bucket, which it always has (optional, as it once might not).
	ObjectStorage *ObjectStorageStats `protobuf:"bytes,13,opt,name=object_storage,json=objectStorage,proto3,oneof" json:"object_storage,omitempty"`
	// Commits: COMMIT requests, and WRITEs' barriers and commits.
	Commits uint64 `protobuf:"varint,14,opt,name=commits,proto3" json:"commits,omitempty"`
	// The nanoseconds they took, all of them.
	CommitNs uint64 `protobuf:"varint,15,opt,name=commit_ns,json=commitNs,proto3" json:"commit_ns,omitempty"`
	// Of those nanoseconds, committing the store.
	CommitStoreNs uint64 `protobuf:"varint,18,opt,name=commit_store_ns,json=commitStoreNs,proto3" json:"commit_store_ns,omitempty"`
	// Of those, waiting for object storage: for the upload of a durable commit, or for the uploads
	// to come within the lag an ordered one allows.
	CommitWaitNs uint64 `protobuf:"varint,20,opt,name=commit_wait_ns,json=commitWaitNs,proto3" json:"commit_wait_ns,omitempty"`
	// Segments stores closed and framed.
	SegmentCloses uint64 `protobuf:"varint,21,opt,name=segment_closes,json=segmentCloses,proto3" json:"segment_closes,omitempty"`
	// The nanoseconds closing them took.
	SegmentCloseNs uint64 `protobuf:"varint,22,opt,name=segment_close_ns,json=segmentCloseNs,proto3" json:"segment_close_ns,omitempty"`
	unknownFields  protoimpl.UnknownFields
	sizeCache      protoimpl.SizeCache
}

func (x *GetStatsResponse) Reset() {
	*x = GetStatsResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[28]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *GetStatsResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*GetStatsResponse) ProtoMessage() {}

func (x *GetStatsResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[28]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use GetStatsResponse.ProtoReflect.Descriptor instead.
func (*GetStatsResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{28}
}

func (x *GetStatsResponse) GetMaps() uint64 {
	if x != nil {
		return x.Maps
	}
	return 0
}

func (x *GetStatsResponse) GetMapNs() uint64 {
	if x != nil {
		return x.MapNs
	}
	return 0
}

func (x *GetStatsResponse) GetFaults() uint64 {
	if x != nil {
		return x.Faults
	}
	return 0
}

func (x *GetStatsResponse) GetFaultNs() uint64 {
	if x != nil {
		return x.FaultNs
	}
	return 0
}

func (x *GetStatsResponse) GetPageIns() uint64 {
	if x != nil {
		return x.PageIns
	}
	return 0
}

func (x *GetStatsResponse) GetPageInBytes() uint64 {
	if x != nil {
		return x.PageInBytes
	}
	return 0
}

func (x *GetStatsResponse) GetFetchedBytes() uint64 {
	if x != nil {
		return x.FetchedBytes
	}
	return 0
}

func (x *GetStatsResponse) GetReads() uint64 {
	if x != nil {
		return x.Reads
	}
	return 0
}

func (x *GetStatsResponse) GetReadBytes() uint64 {
	if x != nil {
		return x.ReadBytes
	}
	return 0
}

func (x *GetStatsResponse) GetReadStoredBytes() uint64 {
	if x != nil {
		return x.ReadStoredBytes
	}
	return 0
}

func (x *GetStatsResponse) GetPrefetches() uint64 {
	if x != nil {
		return x.Prefetches
	}
	return 0
}

func (x *GetStatsResponse) GetUnpacks() uint64 {
	if x != nil {
		return x.Unpacks
	}
	return 0
}

func (x *GetStatsResponse) GetObjectStorage() *ObjectStorageStats {
	if x != nil {
		return x.ObjectStorage
	}
	return nil
}

func (x *GetStatsResponse) GetCommits() uint64 {
	if x != nil {
		return x.Commits
	}
	return 0
}

func (x *GetStatsResponse) GetCommitNs() uint64 {
	if x != nil {
		return x.CommitNs
	}
	return 0
}

func (x *GetStatsResponse) GetCommitStoreNs() uint64 {
	if x != nil {
		return x.CommitStoreNs
	}
	return 0
}

func (x *GetStatsResponse) GetCommitWaitNs() uint64 {
	if x != nil {
		return x.CommitWaitNs
	}
	return 0
}

func (x *GetStatsResponse) GetSegmentCloses() uint64 {
	if x != nil {
		return x.SegmentCloses
	}
	return 0
}

func (x *GetStatsResponse) GetSegmentCloseNs() uint64 {
	if x != nil {
		return x.SegmentCloseNs
	}
	return 0
}

// Asks for a snapshot.
type GetSnapshotRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// How long to wait for the VM processes' answers, in milliseconds: 0 for 2,000; 60,000 at most.
	WaitMs        uint32 `protobuf:"varint,1,opt,name=wait_ms,json=waitMs,proto3" json:"wait_ms,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *GetSnapshotRequest) Reset() {
	*x = GetSnapshotRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[29]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *GetSnapshotRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*GetSnapshotRequest) ProtoMessage() {}

func (x *GetSnapshotRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[29]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use GetSnapshotRequest.ProtoReflect.Descriptor instead.
func (*GetSnapshotRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{29}
}

func (x *GetSnapshotRequest) GetWaitMs() uint32 {
	if x != nil {
		return x.WaitMs
	}
	return 0
}

// What the daemon, its VM processes and their guests counted, by the names its metrics give them.
// Counters only grow, so that a measurement is the difference of two snapshots; gauges are as of
// the snapshot.
type GetSnapshotResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// When the daemon read its own counters, its VM processes having answered or the wait being
	// over: nanoseconds since the Unix epoch.
	TakenUnixNs uint64 `protobuf:"varint,1,opt,name=taken_unix_ns,json=takenUnixNs,proto3" json:"taken_unix_ns,omitempty"`
	// The commit the daemon was built of.
	Build string `protobuf:"bytes,2,opt,name=build,proto3" json:"build,omitempty"`
	// Its counters: those its metrics name fxdax_extentd_NAME_total, as `window_open_maps_refused`.
	Counters map[string]uint64 `protobuf:"bytes,3,rep,name=counters,proto3" json:"counters,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"varint,2,opt,name=value"`
	// Its gauges: those its metrics name fxdax_extentd_NAME.
	Gauges map[string]uint64 `protobuf:"bytes,4,rep,name=gauges,proto3" json:"gauges,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"varint,2,opt,name=value"`
	// Its VM processes that opened a store, in the order they connected.
	Vms           []*VmSnapshot `protobuf:"bytes,5,rep,name=vms,proto3" json:"vms,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *GetSnapshotResponse) Reset() {
	*x = GetSnapshotResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[30]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *GetSnapshotResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*GetSnapshotResponse) ProtoMessage() {}

func (x *GetSnapshotResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[30]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use GetSnapshotResponse.ProtoReflect.Descriptor instead.
func (*GetSnapshotResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{30}
}

func (x *GetSnapshotResponse) GetTakenUnixNs() uint64 {
	if x != nil {
		return x.TakenUnixNs
	}
	return 0
}

func (x *GetSnapshotResponse) GetBuild() string {
	if x != nil {
		return x.Build
	}
	return ""
}

func (x *GetSnapshotResponse) GetCounters() map[string]uint64 {
	if x != nil {
		return x.Counters
	}
	return nil
}

func (x *GetSnapshotResponse) GetGauges() map[string]uint64 {
	if x != nil {
		return x.Gauges
	}
	return nil
}

func (x *GetSnapshotResponse) GetVms() []*VmSnapshot {
	if x != nil {
		return x.Vms
	}
	return nil
}

// A VM process, and its guest, as it answered, or as the daemon has it if it didn't.
type VmSnapshot struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Its connection's number, the daemon's count of connections as it connected: what tells apart
	// a store's writer and its followers, of one branch.
	Connection uint64 `protobuf:"varint,1,opt,name=connection,proto3" json:"connection,omitempty"`
	// Its store's branch.
	Branch uint64 `protobuf:"varint,2,opt,name=branch,proto3" json:"branch,omitempty"`
	// Its store's directory, without symbolic links.
	Path string `protobuf:"bytes,3,opt,name=path,proto3" json:"path,omitempty"`
	// Whether it follows its store, read-only, or writes it.
	Follower bool `protobuf:"varint,4,opt,name=follower,proto3" json:"follower,omitempty"`
	// The commit it said it was built of, empty if it didn't.
	Build string `protobuf:"bytes,5,opt,name=build,proto3" json:"build,omitempty"`
	// Its counters: its window's stage operations, `stage_ops`, their nanoseconds, `stage_ns`, and
	// those that took access away from pages, `stage_overwrites` and `stage_leave_outs`.
	Counters map[string]uint64 `protobuf:"bytes,6,rep,name=counters,proto3" json:"counters,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"varint,2,opt,name=value"`
	// Its gauges (TASK-111.5): the runs of pages its window has installed in its memory, each a
	// mapping the host's vm.max_map_count limits, `host_mappings`, and the most it keeps,
	// `host_mapping_budget`, if it said.
	Gauges map[string]uint64 `protobuf:"bytes,7,rep,name=gauges,proto3" json:"gauges,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"varint,2,opt,name=value"`
	// Its window's, the daemon's, if it has one: `mappings`, `mapped_bytes`, `pages` and
	// `private_pages`.
	Window map[string]uint64 `protobuf:"bytes,8,rep,name=window,proto3" json:"window,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"varint,2,opt,name=value"`
	// Whether it answered within the wait.
	Answered bool `protobuf:"varint,9,opt,name=answered,proto3" json:"answered,omitempty"`
	// How long after it was asked it answered, in nanoseconds: 0 if it didn't.
	AnswerNs uint64 `protobuf:"varint,10,opt,name=answer_ns,json=answerNs,proto3" json:"answer_ns,omitempty"`
	// Its guest's, if a guest answered.
	Guest         *GuestSnapshot `protobuf:"bytes,11,opt,name=guest,proto3,oneof" json:"guest,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *VmSnapshot) Reset() {
	*x = VmSnapshot{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[31]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *VmSnapshot) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*VmSnapshot) ProtoMessage() {}

func (x *VmSnapshot) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[31]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use VmSnapshot.ProtoReflect.Descriptor instead.
func (*VmSnapshot) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{31}
}

func (x *VmSnapshot) GetConnection() uint64 {
	if x != nil {
		return x.Connection
	}
	return 0
}

func (x *VmSnapshot) GetBranch() uint64 {
	if x != nil {
		return x.Branch
	}
	return 0
}

func (x *VmSnapshot) GetPath() string {
	if x != nil {
		return x.Path
	}
	return ""
}

func (x *VmSnapshot) GetFollower() bool {
	if x != nil {
		return x.Follower
	}
	return false
}

func (x *VmSnapshot) GetBuild() string {
	if x != nil {
		return x.Build
	}
	return ""
}

func (x *VmSnapshot) GetCounters() map[string]uint64 {
	if x != nil {
		return x.Counters
	}
	return nil
}

func (x *VmSnapshot) GetGauges() map[string]uint64 {
	if x != nil {
		return x.Gauges
	}
	return nil
}

func (x *VmSnapshot) GetWindow() map[string]uint64 {
	if x != nil {
		return x.Window
	}
	return nil
}

func (x *VmSnapshot) GetAnswered() bool {
	if x != nil {
		return x.Answered
	}
	return false
}

func (x *VmSnapshot) GetAnswerNs() uint64 {
	if x != nil {
		return x.AnswerNs
	}
	return 0
}

func (x *VmSnapshot) GetGuest() *GuestSnapshot {
	if x != nil {
		return x.Guest
	}
	return nil
}

// What a guest's Fxfs counted, as its /proc/fs/fxfs/TAG says.
type GuestSnapshot struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The commit it was built of, empty if it didn't say.
	Build string `protobuf:"bytes,1,opt,name=build,proto3" json:"build,omitempty"`
	// Its counters and gauges.
	Counters map[string]uint64 `protobuf:"bytes,2,rep,name=counters,proto3" json:"counters,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"varint,2,opt,name=value"`
	// Its histograms, by name without `_hist`.
	Histograms    map[string]*Histogram `protobuf:"bytes,3,rep,name=histograms,proto3" json:"histograms,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"bytes,2,opt,name=value"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *GuestSnapshot) Reset() {
	*x = GuestSnapshot{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[32]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *GuestSnapshot) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*GuestSnapshot) ProtoMessage() {}

func (x *GuestSnapshot) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[32]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use GuestSnapshot.ProtoReflect.Descriptor instead.
func (*GuestSnapshot) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{32}
}

func (x *GuestSnapshot) GetBuild() string {
	if x != nil {
		return x.Build
	}
	return ""
}

func (x *GuestSnapshot) GetCounters() map[string]uint64 {
	if x != nil {
		return x.Counters
	}
	return nil
}

func (x *GuestSnapshot) GetHistograms() map[string]*Histogram {
	if x != nil {
		return x.Histograms
	}
	return nil
}

// A histogram's buckets' counts, in order.
type Histogram struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Each bucket's count.
	Buckets       []uint64 `protobuf:"varint,1,rep,packed,name=buckets,proto3" json:"buckets,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *Histogram) Reset() {
	*x = Histogram{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[33]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *Histogram) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*Histogram) ProtoMessage() {}

func (x *Histogram) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[33]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use Histogram.ProtoReflect.Descriptor instead.
func (*Histogram) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{33}
}

func (x *Histogram) GetBuckets() []uint64 {
	if x != nil {
		return x.Buckets
	}
	return nil
}

// What the daemon did with its bucket.
type ObjectStorageStats struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Requests, by kind (`get`, `get-range`, `head`, `list`, `create`, `update`, `overwrite`,
	// `delete`).
	Requests map[string]uint64 `protobuf:"bytes,1,rep,name=requests,proto3" json:"requests,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"varint,2,opt,name=value"`
	// Bytes uploaded, by kind of object.
	UploadedBytes map[string]uint64 `protobuf:"bytes,2,rep,name=uploaded_bytes,json=uploadedBytes,proto3" json:"uploaded_bytes,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"varint,2,opt,name=value"`
	// Bytes downloaded.
	DownloadedBytes uint64 `protobuf:"varint,3,opt,name=downloaded_bytes,json=downloadedBytes,proto3" json:"downloaded_bytes,omitempty"`
	// Requests that failed, unanswered, by kind, as `requests`: each is retried until the bucket
	// answers it, its error in the daemon's log (TASK-314). Not those the bucket answers with no
	// object, one there, or another version.
	Failures      map[string]uint64 `protobuf:"bytes,4,rep,name=failures,proto3" json:"failures,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"varint,2,opt,name=value"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ObjectStorageStats) Reset() {
	*x = ObjectStorageStats{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[34]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ObjectStorageStats) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ObjectStorageStats) ProtoMessage() {}

func (x *ObjectStorageStats) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[34]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ObjectStorageStats.ProtoReflect.Descriptor instead.
func (*ObjectStorageStats) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{34}
}

func (x *ObjectStorageStats) GetRequests() map[string]uint64 {
	if x != nil {
		return x.Requests
	}
	return nil
}

func (x *ObjectStorageStats) GetUploadedBytes() map[string]uint64 {
	if x != nil {
		return x.UploadedBytes
	}
	return nil
}

func (x *ObjectStorageStats) GetDownloadedBytes() uint64 {
	if x != nil {
		return x.DownloadedBytes
	}
	return 0
}

func (x *ObjectStorageStats) GetFailures() map[string]uint64 {
	if x != nil {
		return x.Failures
	}
	return nil
}

// Grants a tenant.
type GrantRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Its name: letters, digits, `-`, `_` and `.`, not first, at most 64.
	Tenant string `protobuf:"bytes,1,opt,name=tenant,proto3" json:"tenant,omitempty"`
	// The directory its stores are in, which must exist: its VM processes name stores in it
	// alone.
	Dir string `protobuf:"bytes,2,opt,name=dir,proto3" json:"dir,omitempty"`
	// The user to give its socket to, its jails': the socket is theirs and its group's alone. The
	// daemon's own user, if not given.
	Uid *uint32 `protobuf:"varint,3,opt,name=uid,proto3,oneof" json:"uid,omitempty"`
	// The group to give its socket to; the daemon's own, if not given.
	Gid *uint32 `protobuf:"varint,4,opt,name=gid,proto3,oneof" json:"gid,omitempty"`
	// Its ID in the bucket, not 0 (TASK-364): the stores in its directory are volumes of its cells,
	// under its prefix, and its VM processes open no store of another tenant's cells
	// (PERMISSION_DENIED, EACCES). Granting it again with another ID is refused (ALREADY_EXISTS).
	Id            uint64 `protobuf:"varint,5,opt,name=id,proto3" json:"id,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *GrantRequest) Reset() {
	*x = GrantRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[35]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *GrantRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*GrantRequest) ProtoMessage() {}

func (x *GrantRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[35]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use GrantRequest.ProtoReflect.Descriptor instead.
func (*GrantRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{35}
}

func (x *GrantRequest) GetTenant() string {
	if x != nil {
		return x.Tenant
	}
	return ""
}

func (x *GrantRequest) GetDir() string {
	if x != nil {
		return x.Dir
	}
	return ""
}

func (x *GrantRequest) GetUid() uint32 {
	if x != nil && x.Uid != nil {
		return *x.Uid
	}
	return 0
}

func (x *GrantRequest) GetGid() uint32 {
	if x != nil && x.Gid != nil {
		return *x.Gid
	}
	return 0
}

func (x *GrantRequest) GetId() uint64 {
	if x != nil {
		return x.Id
	}
	return 0
}

// The tenant's VM socket.
type GrantResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Its path, on the daemon's host.
	Socket        string `protobuf:"bytes,1,opt,name=socket,proto3" json:"socket,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *GrantResponse) Reset() {
	*x = GrantResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[36]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *GrantResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*GrantResponse) ProtoMessage() {}

func (x *GrantResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[36]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use GrantResponse.ProtoReflect.Descriptor instead.
func (*GrantResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{36}
}

func (x *GrantResponse) GetSocket() string {
	if x != nil {
		return x.Socket
	}
	return ""
}

// Revokes a tenant's grant.
type RevokeRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The tenant, granted (NOT_FOUND otherwise).
	Tenant        string `protobuf:"bytes,1,opt,name=tenant,proto3" json:"tenant,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *RevokeRequest) Reset() {
	*x = RevokeRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[37]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *RevokeRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*RevokeRequest) ProtoMessage() {}

func (x *RevokeRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[37]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use RevokeRequest.ProtoReflect.Descriptor instead.
func (*RevokeRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{37}
}

func (x *RevokeRequest) GetTenant() string {
	if x != nil {
		return x.Tenant
	}
	return ""
}

// The tenant's grant revoked.
type RevokeResponse struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *RevokeResponse) Reset() {
	*x = RevokeResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[38]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *RevokeResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*RevokeResponse) ProtoMessage() {}

func (x *RevokeResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[38]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use RevokeResponse.ProtoReflect.Descriptor instead.
func (*RevokeResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{38}
}

// Limits the memory of a tenant, or of every tenant.
type SetMemoryLimitRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The tenant whose memory to limit, alive now or granted later, until its limit is set again;
	// empty for every tenant's together, the default tenant's with them. A tenant revoked and
	// granted again while its earlier VMs run has the limit for each of its grants' VMs.
	Tenant string `protobuf:"bytes,1,opt,name=tenant,proto3" json:"tenant,omitempty"`
	// The limit, in bytes, rounded down to 2 MiB, and at least that (INVALID_ARGUMENT otherwise);
	// absent, none.
	Bytes         *uint64 `protobuf:"varint,2,opt,name=bytes,proto3,oneof" json:"bytes,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SetMemoryLimitRequest) Reset() {
	*x = SetMemoryLimitRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[39]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SetMemoryLimitRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SetMemoryLimitRequest) ProtoMessage() {}

func (x *SetMemoryLimitRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[39]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SetMemoryLimitRequest.ProtoReflect.Descriptor instead.
func (*SetMemoryLimitRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{39}
}

func (x *SetMemoryLimitRequest) GetTenant() string {
	if x != nil {
		return x.Tenant
	}
	return ""
}

func (x *SetMemoryLimitRequest) GetBytes() uint64 {
	if x != nil && x.Bytes != nil {
		return *x.Bytes
	}
	return 0
}

// The limit is set.
type SetMemoryLimitResponse struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SetMemoryLimitResponse) Reset() {
	*x = SetMemoryLimitResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[40]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SetMemoryLimitResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SetMemoryLimitResponse) ProtoMessage() {}

func (x *SetMemoryLimitResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[40]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SetMemoryLimitResponse.ProtoReflect.Descriptor instead.
func (*SetMemoryLimitResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{40}
}

// Weighs a tenant's memory.
type SetMemoryWeightRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The tenant, alive now or granted later, until its weight is set again (INVALID_ARGUMENT for a
	// malformed name).
	Tenant string `protobuf:"bytes,1,opt,name=tenant,proto3" json:"tenant,omitempty"`
	// Its weight: 0 for no share kept under pressure.
	Weight        uint64 `protobuf:"varint,2,opt,name=weight,proto3" json:"weight,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SetMemoryWeightRequest) Reset() {
	*x = SetMemoryWeightRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[41]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SetMemoryWeightRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SetMemoryWeightRequest) ProtoMessage() {}

func (x *SetMemoryWeightRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[41]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SetMemoryWeightRequest.ProtoReflect.Descriptor instead.
func (*SetMemoryWeightRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{41}
}

func (x *SetMemoryWeightRequest) GetTenant() string {
	if x != nil {
		return x.Tenant
	}
	return ""
}

func (x *SetMemoryWeightRequest) GetWeight() uint64 {
	if x != nil {
		return x.Weight
	}
	return 0
}

// The weight is set.
type SetMemoryWeightResponse struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SetMemoryWeightResponse) Reset() {
	*x = SetMemoryWeightResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[42]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SetMemoryWeightResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SetMemoryWeightResponse) ProtoMessage() {}

func (x *SetMemoryWeightResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[42]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SetMemoryWeightResponse.ProtoReflect.Descriptor instead.
func (*SetMemoryWeightResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{42}
}

// Sets the room of a tenant's connections.
type SetTenantRoomRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The tenant whose room to set, alive now or granted later, until it is set again; empty for
	// every tenant's whose own isn't set, the default tenant's with them.
	Tenant string `protobuf:"bytes,1,opt,name=tenant,proto3" json:"tenant,omitempty"`
	// The room, in bytes, a payload's most, 64 MiB, at least (INVALID_ARGUMENT otherwise); absent,
	// every tenant's, or for every tenant the default.
	Bytes         *uint64 `protobuf:"varint,2,opt,name=bytes,proto3,oneof" json:"bytes,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SetTenantRoomRequest) Reset() {
	*x = SetTenantRoomRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[43]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SetTenantRoomRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SetTenantRoomRequest) ProtoMessage() {}

func (x *SetTenantRoomRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[43]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SetTenantRoomRequest.ProtoReflect.Descriptor instead.
func (*SetTenantRoomRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{43}
}

func (x *SetTenantRoomRequest) GetTenant() string {
	if x != nil {
		return x.Tenant
	}
	return ""
}

func (x *SetTenantRoomRequest) GetBytes() uint64 {
	if x != nil && x.Bytes != nil {
		return *x.Bytes
	}
	return 0
}

// The room is set.
type SetTenantRoomResponse struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SetTenantRoomResponse) Reset() {
	*x = SetTenantRoomResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[44]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SetTenantRoomResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SetTenantRoomResponse) ProtoMessage() {}

func (x *SetTenantRoomResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[44]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SetTenantRoomResponse.ProtoReflect.Descriptor instead.
func (*SetTenantRoomResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{44}
}

// Evicts pages toward a target.
type ShrinkMemoryRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The tenant whose memory to shrink, alive (NOT_FOUND otherwise); empty for every tenant's.
	Tenant string `protobuf:"bytes,1,opt,name=tenant,proto3" json:"tenant,omitempty"`
	// The most bytes to hold.
	TargetBytes   uint64 `protobuf:"varint,2,opt,name=target_bytes,json=targetBytes,proto3" json:"target_bytes,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ShrinkMemoryRequest) Reset() {
	*x = ShrinkMemoryRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[45]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ShrinkMemoryRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ShrinkMemoryRequest) ProtoMessage() {}

func (x *ShrinkMemoryRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[45]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ShrinkMemoryRequest.ProtoReflect.Descriptor instead.
func (*ShrinkMemoryRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{45}
}

func (x *ShrinkMemoryRequest) GetTenant() string {
	if x != nil {
		return x.Tenant
	}
	return ""
}

func (x *ShrinkMemoryRequest) GetTargetBytes() uint64 {
	if x != nil {
		return x.TargetBytes
	}
	return 0
}

// What is held once it shrank.
type ShrinkMemoryResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The bytes held: above the target if nothing more could be evicted.
	Bytes         uint64 `protobuf:"varint,1,opt,name=bytes,proto3" json:"bytes,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ShrinkMemoryResponse) Reset() {
	*x = ShrinkMemoryResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[46]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ShrinkMemoryResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ShrinkMemoryResponse) ProtoMessage() {}

func (x *ShrinkMemoryResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[46]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ShrinkMemoryResponse.ProtoReflect.Descriptor instead.
func (*ShrinkMemoryResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{46}
}

func (x *ShrinkMemoryResponse) GetBytes() uint64 {
	if x != nil {
		return x.Bytes
	}
	return 0
}

// Limits the disk of a tenant, or of every tenant.
type SetDiskLimitRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The tenant whose disk to limit, alive now or granted later, until its limit is set again;
	// empty for every tenant's together. A tenant revoked and granted again while its earlier VMs
	// run has the limit for each of its directories.
	Tenant string `protobuf:"bytes,1,opt,name=tenant,proto3" json:"tenant,omitempty"`
	// The limit, in bytes; absent, none.
	Bytes         *uint64 `protobuf:"varint,2,opt,name=bytes,proto3,oneof" json:"bytes,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SetDiskLimitRequest) Reset() {
	*x = SetDiskLimitRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[47]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SetDiskLimitRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SetDiskLimitRequest) ProtoMessage() {}

func (x *SetDiskLimitRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[47]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SetDiskLimitRequest.ProtoReflect.Descriptor instead.
func (*SetDiskLimitRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{47}
}

func (x *SetDiskLimitRequest) GetTenant() string {
	if x != nil {
		return x.Tenant
	}
	return ""
}

func (x *SetDiskLimitRequest) GetBytes() uint64 {
	if x != nil && x.Bytes != nil {
		return *x.Bytes
	}
	return 0
}

// The limit is set.
type SetDiskLimitResponse struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SetDiskLimitResponse) Reset() {
	*x = SetDiskLimitResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[48]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SetDiskLimitResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SetDiskLimitResponse) ProtoMessage() {}

func (x *SetDiskLimitResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[48]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SetDiskLimitResponse.ProtoReflect.Descriptor instead.
func (*SetDiskLimitResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{48}
}

// Asks what a tenant's disk takes.
type GetTenantDiskRequest struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The tenant, alive (NOT_FOUND otherwise); empty for every tenant together.
	Tenant        string `protobuf:"bytes,1,opt,name=tenant,proto3" json:"tenant,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *GetTenantDiskRequest) Reset() {
	*x = GetTenantDiskRequest{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[49]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *GetTenantDiskRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*GetTenantDiskRequest) ProtoMessage() {}

func (x *GetTenantDiskRequest) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[49]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use GetTenantDiskRequest.ProtoReflect.Descriptor instead.
func (*GetTenantDiskRequest) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{49}
}

func (x *GetTenantDiskRequest) GetTenant() string {
	if x != nil {
		return x.Tenant
	}
	return ""
}

// What a tenant's disk takes.
type GetTenantDiskResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The bytes of the filesystem's blocks its directories' files take, as `du -sx` counts them,
	// with what writes admitted add as the count is made.
	Bytes uint64 `protobuf:"varint,1,opt,name=bytes,proto3" json:"bytes,omitempty"`
	// Its limit, if it has one.
	LimitBytes    *uint64 `protobuf:"varint,2,opt,name=limit_bytes,json=limitBytes,proto3,oneof" json:"limit_bytes,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *GetTenantDiskResponse) Reset() {
	*x = GetTenantDiskResponse{}
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[50]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *GetTenantDiskResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*GetTenantDiskResponse) ProtoMessage() {}

func (x *GetTenantDiskResponse) ProtoReflect() protoreflect.Message {
	mi := &file_fxdax_extentd_v1_extentd_proto_msgTypes[50]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use GetTenantDiskResponse.ProtoReflect.Descriptor instead.
func (*GetTenantDiskResponse) Descriptor() ([]byte, []int) {
	return file_fxdax_extentd_v1_extentd_proto_rawDescGZIP(), []int{50}
}

func (x *GetTenantDiskResponse) GetBytes() uint64 {
	if x != nil {
		return x.Bytes
	}
	return 0
}

func (x *GetTenantDiskResponse) GetLimitBytes() uint64 {
	if x != nil && x.LimitBytes != nil {
		return *x.LimitBytes
	}
	return 0
}

var File_fxdax_extentd_v1_extentd_proto protoreflect.FileDescriptor

const file_fxdax_extentd_v1_extentd_proto_rawDesc = "" +
	"\n" +
	"\x1efxdax/extentd/v1/extentd.proto\x12\x10fxdax.extentd.v1\"\xdf\x01\n" +
	"\x12CreateStoreRequest\x12\x12\n" +
	"\x04path\x18\x01 \x01(\tR\x04path\x12\x1a\n" +
	"\bcapacity\x18\x02 \x01(\x04R\bcapacity\x12\x16\n" +
	"\x06branch\x18\x03 \x01(\x04R\x06branch\x12\x19\n" +
	"\x05token\x18\x04 \x01(\x04H\x00R\x05token\x88\x01\x01\x12\x16\n" +
	"\x06tenant\x18\x05 \x01(\x04R\x06tenant\x12\x12\n" +
	"\x04cell\x18\x06 \x01(\x04R\x04cell\x120\n" +
	"\x04kind\x18\a \x01(\x0e2\x1c.fxdax.extentd.v1.VolumeKindR\x04kindB\b\n" +
	"\x06_token\"Y\n" +
	"\x13CreateStoreResponse\x12\x16\n" +
	"\x06branch\x18\x01 \x01(\x04R\x06branch\x12\x16\n" +
	"\x06tenant\x18\x02 \x01(\x04R\x06tenant\x12\x12\n" +
	"\x04cell\x18\x03 \x01(\x04R\x04cell\"\x90\x01\n" +
	"\x12ImportImageRequest\x12\x12\n" +
	"\x04path\x18\x01 \x01(\tR\x04path\x12\x1a\n" +
	"\bcapacity\x18\x02 \x01(\x04R\bcapacity\x12\x14\n" +
	"\x05image\x18\x03 \x01(\tR\x05image\x12\x1c\n" +
	"\treference\x18\x04 \x01(\tR\treference\x12\x16\n" +
	"\x06tenant\x18\x05 \x01(\x04R\x06tenant\"Y\n" +
	"\x13ImportImageResponse\x12\x16\n" +
	"\x06branch\x18\x01 \x01(\x04R\x06branch\x12\x16\n" +
	"\x06tenant\x18\x02 \x01(\x04R\x06tenant\x12\x12\n" +
	"\x04cell\x18\x03 \x01(\x04R\x04cell\"j\n" +
	"\x10ForkStoreRequest\x12\x12\n" +
	"\x04from\x18\x01 \x01(\tR\x04from\x12\x0e\n" +
	"\x02to\x18\x02 \x01(\tR\x02to\x12\x14\n" +
	"\x05point\x18\x04 \x01(\bR\x05point\x12\x16\n" +
	"\x06shared\x18\x05 \x01(\bR\x06sharedJ\x04\b\x03\x10\x04\"W\n" +
	"\x11ForkStoreResponse\x12\x16\n" +
	"\x06branch\x18\x01 \x01(\x04R\x06branch\x12\x16\n" +
	"\x06tenant\x18\x02 \x01(\x04R\x06tenant\x12\x12\n" +
	"\x04cell\x18\x03 \x01(\x04R\x04cell\"(\n" +
	"\x12DeleteStoreRequest\x12\x12\n" +
	"\x04path\x18\x01 \x01(\tR\x04path\"\x15\n" +
	"\x13DeleteStoreResponse\"&\n" +
	"\x10DropStoreRequest\x12\x12\n" +
	"\x04path\x18\x01 \x01(\tR\x04path\"\x13\n" +
	"\x11DropStoreResponse\"D\n" +
	"\x12ResizeStoreRequest\x12\x12\n" +
	"\x04path\x18\x01 \x01(\tR\x04path\x12\x1a\n" +
	"\bcapacity\x18\x02 \x01(\x04R\bcapacity\"\x15\n" +
	"\x13ResizeStoreResponse\"=\n" +
	"\x11PlaceStoreRequest\x12\x12\n" +
	"\x04path\x18\x01 \x01(\tR\x04path\x12\x14\n" +
	"\x05token\x18\x02 \x01(\x04R\x05token\"\x14\n" +
	"\x12PlaceStoreResponse\"?\n" +
	"\x11SharePointRequest\x12\x12\n" +
	"\x04path\x18\x01 \x01(\tR\x04path\x12\x16\n" +
	"\x06shared\x18\x02 \x01(\bR\x06shared\"\x14\n" +
	"\x12SharePointResponse\"G\n" +
	"\x11MakePublicRequest\x12\x12\n" +
	"\x04path\x18\x01 \x01(\tR\x04path\x12\x1e\n" +
	"\n" +
	"generation\x18\x02 \x01(\bR\n" +
	"generation\"\x14\n" +
	"\x12MakePublicResponse\"=\n" +
	"\x11AttachBaseRequest\x12\x14\n" +
	"\x05store\x18\x01 \x01(\tR\x05store\x12\x12\n" +
	"\x04base\x18\x02 \x01(\tR\x04base\"\x14\n" +
	"\x12AttachBaseResponse\"&\n" +
	"\x10SyncStoreRequest\x12\x12\n" +
	"\x04path\x18\x01 \x01(\tR\x04path\"\x13\n" +
	"\x11SyncStoreResponse\"&\n" +
	"\x10StatStoreRequest\x12\x12\n" +
	"\x04path\x18\x01 \x01(\tR\x04path\"\xa8\x04\n" +
	"\x11StatStoreResponse\x12\x16\n" +
	"\x06branch\x18\x01 \x01(\x04R\x06branch\x12\x1a\n" +
	"\bcapacity\x18\x02 \x01(\x04R\bcapacity\x12\x12\n" +
	"\x04used\x18\x03 \x01(\x04R\x04used\x12\x1c\n" +
	"\tavailable\x18\x04 \x01(\x04R\tavailable\x12\x16\n" +
	"\x06public\x18\x05 \x01(\bR\x06public\x12\x12\n" +
	"\x04open\x18\x06 \x01(\bR\x04open\x12\x10\n" +
	"\x03own\x18\a \x01(\x04R\x03own\x12\x1c\n" +
	"\tinherited\x18\b \x01(\x04R\tinherited\x12\x1c\n" +
	"\tcommitted\x18\t \x01(\x04R\tcommitted\x12\x1c\n" +
	"\tuploading\x18\n" +
	" \x01(\bR\tuploading\x12\x1f\n" +
	"\vlease_token\x18\v \x01(\x04R\n" +
	"leaseToken\x12\x1f\n" +
	"\vlease_epoch\x18\f \x01(\x04R\n" +
	"leaseEpoch\x12-\n" +
	"\x13lease_taken_unix_ms\x18\r \x01(\x04R\x10leaseTakenUnixMs\x12\x1a\n" +
	"\buploaded\x18\x0e \x01(\x04R\buploaded\x120\n" +
	"\x14pending_upload_bytes\x18\x0f \x01(\x04R\x12pendingUploadBytes\x12*\n" +
	"\x11oldest_pending_ms\x18\x10 \x01(\x04R\x0foldestPendingMs\x12\x16\n" +
	"\x06tenant\x18\x11 \x01(\x04R\x06tenant\x12\x12\n" +
	"\x04cell\x18\x12 \x01(\x04R\x04cell\"\x13\n" +
	"\x11ListStoresRequest\"K\n" +
	"\x12ListStoresResponse\x125\n" +
	"\x06stores\x18\x01 \x03(\v2\x1d.fxdax.extentd.v1.ListedStoreR\x06stores\"Z\n" +
	"\vListedStore\x12\x12\n" +
	"\x04path\x18\x01 \x01(\tR\x04path\x127\n" +
	"\x04stat\x18\x02 \x01(\v2#.fxdax.extentd.v1.StatStoreResponseR\x04stat\"\x11\n" +
	"\x0fGetStatsRequest\"\xf3\x05\n" +
	"\x10GetStatsResponse\x12\x12\n" +
	"\x04maps\x18\x01 \x01(\x04R\x04maps\x12\x15\n" +
	"\x06map_ns\x18\x02 \x01(\x04R\x05mapNs\x12\x16\n" +
	"\x06faults\x18\x03 \x01(\x04R\x06faults\x12\x19\n" +
	"\bfault_ns\x18\x04 \x01(\x04R\afaultNs\x12\x19\n" +
	"\bpage_ins\x18\x05 \x01(\x04R\apageIns\x12\"\n" +
	"\rpage_in_bytes\x18\x06 \x01(\x04R\vpageInBytes\x12#\n" +
	"\rfetched_bytes\x18\a \x01(\x04R\ffetchedBytes\x12\x14\n" +
	"\x05reads\x18\b \x01(\x04R\x05reads\x12\x1d\n" +
	"\n" +
	"read_bytes\x18\t \x01(\x04R\treadBytes\x12*\n" +
	"\x11read_stored_bytes\x18\n" +
	" \x01(\x04R\x0freadStoredBytes\x12\x1e\n" +
	"\n" +
	"prefetches\x18\v \x01(\x04R\n" +
	"prefetches\x12\x18\n" +
	"\aunpacks\x18\f \x01(\x04R\aunpacks\x12P\n" +
	"\x0eobject_storage\x18\r \x01(\v2$.fxdax.extentd.v1.ObjectStorageStatsH\x00R\robjectStorage\x88\x01\x01\x12\x18\n" +
	"\acommits\x18\x0e \x01(\x04R\acommits\x12\x1b\n" +
	"\tcommit_ns\x18\x0f \x01(\x04R\bcommitNs\x12&\n" +
	"\x0fcommit_store_ns\x18\x12 \x01(\x04R\rcommitStoreNs\x12$\n" +
	"\x0ecommit_wait_ns\x18\x14 \x01(\x04R\fcommitWaitNs\x12%\n" +
	"\x0esegment_closes\x18\x15 \x01(\x04R\rsegmentCloses\x12(\n" +
	"\x10segment_close_ns\x18\x16 \x01(\x04R\x0esegmentCloseNsB\x11\n" +
	"\x0f_object_storageJ\x04\b\x10\x10\x11J\x04\b\x11\x10\x12J\x04\b\x13\x10\x14R\x0fcommit_write_nsR\x14commit_written_bytesR\x0ecommit_seal_ns\"-\n" +
	"\x12GetSnapshotRequest\x12\x17\n" +
	"\await_ms\x18\x01 \x01(\rR\x06waitMs\"\x93\x03\n" +
	"\x13GetSnapshotResponse\x12\"\n" +
	"\rtaken_unix_ns\x18\x01 \x01(\x04R\vtakenUnixNs\x12\x14\n" +
	"\x05build\x18\x02 \x01(\tR\x05build\x12O\n" +
	"\bcounters\x18\x03 \x03(\v23.fxdax.extentd.v1.GetSnapshotResponse.CountersEntryR\bcounters\x12I\n" +
	"\x06gauges\x18\x04 \x03(\v21.fxdax.extentd.v1.GetSnapshotResponse.GaugesEntryR\x06gauges\x12.\n" +
	"\x03vms\x18\x05 \x03(\v2\x1c.fxdax.extentd.v1.VmSnapshotR\x03vms\x1a;\n" +
	"\rCountersEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12\x14\n" +
	"\x05value\x18\x02 \x01(\x04R\x05value:\x028\x01\x1a9\n" +
	"\vGaugesEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12\x14\n" +
	"\x05value\x18\x02 \x01(\x04R\x05value:\x028\x01\"\x88\x05\n" +
	"\n" +
	"VmSnapshot\x12\x1e\n" +
	"\n" +
	"connection\x18\x01 \x01(\x04R\n" +
	"connection\x12\x16\n" +
	"\x06branch\x18\x02 \x01(\x04R\x06branch\x12\x12\n" +
	"\x04path\x18\x03 \x01(\tR\x04path\x12\x1a\n" +
	"\bfollower\x18\x04 \x01(\bR\bfollower\x12\x14\n" +
	"\x05build\x18\x05 \x01(\tR\x05build\x12F\n" +
	"\bcounters\x18\x06 \x03(\v2*.fxdax.extentd.v1.VmSnapshot.CountersEntryR\bcounters\x12@\n" +
	"\x06gauges\x18\a \x03(\v2(.fxdax.extentd.v1.VmSnapshot.GaugesEntryR\x06gauges\x12@\n" +
	"\x06window\x18\b \x03(\v2(.fxdax.extentd.v1.VmSnapshot.WindowEntryR\x06window\x12\x1a\n" +
	"\banswered\x18\t \x01(\bR\banswered\x12\x1b\n" +
	"\tanswer_ns\x18\n" +
	" \x01(\x04R\banswerNs\x12:\n" +
	"\x05guest\x18\v \x01(\v2\x1f.fxdax.extentd.v1.GuestSnapshotH\x00R\x05guest\x88\x01\x01\x1a;\n" +
	"\rCountersEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12\x14\n" +
	"\x05value\x18\x02 \x01(\x04R\x05value:\x028\x01\x1a9\n" +
	"\vGaugesEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12\x14\n" +
	"\x05value\x18\x02 \x01(\x04R\x05value:\x028\x01\x1a9\n" +
	"\vWindowEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12\x14\n" +
	"\x05value\x18\x02 \x01(\x04R\x05value:\x028\x01B\b\n" +
	"\x06_guest\"\xda\x02\n" +
	"\rGuestSnapshot\x12\x14\n" +
	"\x05build\x18\x01 \x01(\tR\x05build\x12I\n" +
	"\bcounters\x18\x02 \x03(\v2-.fxdax.extentd.v1.GuestSnapshot.CountersEntryR\bcounters\x12O\n" +
	"\n" +
	"histograms\x18\x03 \x03(\v2/.fxdax.extentd.v1.GuestSnapshot.HistogramsEntryR\n" +
	"histograms\x1a;\n" +
	"\rCountersEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12\x14\n" +
	"\x05value\x18\x02 \x01(\x04R\x05value:\x028\x01\x1aZ\n" +
	"\x0fHistogramsEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x121\n" +
	"\x05value\x18\x02 \x01(\v2\x1b.fxdax.extentd.v1.HistogramR\x05value:\x028\x01\"%\n" +
	"\tHistogram\x12\x18\n" +
	"\abuckets\x18\x01 \x03(\x04R\abuckets\"\xfb\x03\n" +
	"\x12ObjectStorageStats\x12N\n" +
	"\brequests\x18\x01 \x03(\v22.fxdax.extentd.v1.ObjectStorageStats.RequestsEntryR\brequests\x12^\n" +
	"\x0euploaded_bytes\x18\x02 \x03(\v27.fxdax.extentd.v1.ObjectStorageStats.UploadedBytesEntryR\ruploadedBytes\x12)\n" +
	"\x10downloaded_bytes\x18\x03 \x01(\x04R\x0fdownloadedBytes\x12N\n" +
	"\bfailures\x18\x04 \x03(\v22.fxdax.extentd.v1.ObjectStorageStats.FailuresEntryR\bfailures\x1a;\n" +
	"\rRequestsEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12\x14\n" +
	"\x05value\x18\x02 \x01(\x04R\x05value:\x028\x01\x1a@\n" +
	"\x12UploadedBytesEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12\x14\n" +
	"\x05value\x18\x02 \x01(\x04R\x05value:\x028\x01\x1a;\n" +
	"\rFailuresEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12\x14\n" +
	"\x05value\x18\x02 \x01(\x04R\x05value:\x028\x01\"\x86\x01\n" +
	"\fGrantRequest\x12\x16\n" +
	"\x06tenant\x18\x01 \x01(\tR\x06tenant\x12\x10\n" +
	"\x03dir\x18\x02 \x01(\tR\x03dir\x12\x15\n" +
	"\x03uid\x18\x03 \x01(\rH\x00R\x03uid\x88\x01\x01\x12\x15\n" +
	"\x03gid\x18\x04 \x01(\rH\x01R\x03gid\x88\x01\x01\x12\x0e\n" +
	"\x02id\x18\x05 \x01(\x04R\x02idB\x06\n" +
	"\x04_uidB\x06\n" +
	"\x04_gid\"'\n" +
	"\rGrantResponse\x12\x16\n" +
	"\x06socket\x18\x01 \x01(\tR\x06socket\"'\n" +
	"\rRevokeRequest\x12\x16\n" +
	"\x06tenant\x18\x01 \x01(\tR\x06tenant\"\x10\n" +
	"\x0eRevokeResponse\"T\n" +
	"\x15SetMemoryLimitRequest\x12\x16\n" +
	"\x06tenant\x18\x01 \x01(\tR\x06tenant\x12\x19\n" +
	"\x05bytes\x18\x02 \x01(\x04H\x00R\x05bytes\x88\x01\x01B\b\n" +
	"\x06_bytes\"\x18\n" +
	"\x16SetMemoryLimitResponse\"H\n" +
	"\x16SetMemoryWeightRequest\x12\x16\n" +
	"\x06tenant\x18\x01 \x01(\tR\x06tenant\x12\x16\n" +
	"\x06weight\x18\x02 \x01(\x04R\x06weight\"\x19\n" +
	"\x17SetMemoryWeightResponse\"S\n" +
	"\x14SetTenantRoomRequest\x12\x16\n" +
	"\x06tenant\x18\x01 \x01(\tR\x06tenant\x12\x19\n" +
	"\x05bytes\x18\x02 \x01(\x04H\x00R\x05bytes\x88\x01\x01B\b\n" +
	"\x06_bytes\"\x17\n" +
	"\x15SetTenantRoomResponse\"P\n" +
	"\x13ShrinkMemoryRequest\x12\x16\n" +
	"\x06tenant\x18\x01 \x01(\tR\x06tenant\x12!\n" +
	"\ftarget_bytes\x18\x02 \x01(\x04R\vtargetBytes\",\n" +
	"\x14ShrinkMemoryResponse\x12\x14\n" +
	"\x05bytes\x18\x01 \x01(\x04R\x05bytes\"R\n" +
	"\x13SetDiskLimitRequest\x12\x16\n" +
	"\x06tenant\x18\x01 \x01(\tR\x06tenant\x12\x19\n" +
	"\x05bytes\x18\x02 \x01(\x04H\x00R\x05bytes\x88\x01\x01B\b\n" +
	"\x06_bytes\"\x16\n" +
	"\x14SetDiskLimitResponse\".\n" +
	"\x14GetTenantDiskRequest\x12\x16\n" +
	"\x06tenant\x18\x01 \x01(\tR\x06tenant\"c\n" +
	"\x15GetTenantDiskResponse\x12\x14\n" +
	"\x05bytes\x18\x01 \x01(\x04R\x05bytes\x12$\n" +
	"\vlimit_bytes\x18\x02 \x01(\x04H\x00R\n" +
	"limitBytes\x88\x01\x01B\x0e\n" +
	"\f_limit_bytes*:\n" +
	"\n" +
	"VolumeKind\x12\x14\n" +
	"\x10VOLUME_KIND_FXFS\x10\x00\x12\x16\n" +
	"\x12VOLUME_KIND_SQLITE\x10\x012\xa3\x10\n" +
	"\aExtentd\x12Z\n" +
	"\vCreateStore\x12$.fxdax.extentd.v1.CreateStoreRequest\x1a%.fxdax.extentd.v1.CreateStoreResponse\x12Z\n" +
	"\vImportImage\x12$.fxdax.extentd.v1.ImportImageRequest\x1a%.fxdax.extentd.v1.ImportImageResponse\x12T\n" +
	"\tForkStore\x12\".fxdax.extentd.v1.ForkStoreRequest\x1a#.fxdax.extentd.v1.ForkStoreResponse\x12Z\n" +
	"\vDeleteStore\x12$.fxdax.extentd.v1.DeleteStoreRequest\x1a%.fxdax.extentd.v1.DeleteStoreResponse\x12T\n" +
	"\tDropStore\x12\".fxdax.extentd.v1.DropStoreRequest\x1a#.fxdax.extentd.v1.DropStoreResponse\x12Z\n" +
	"\vResizeStore\x12$.fxdax.extentd.v1.ResizeStoreRequest\x1a%.fxdax.extentd.v1.ResizeStoreResponse\x12W\n" +
	"\n" +
	"PlaceStore\x12#.fxdax.extentd.v1.PlaceStoreRequest\x1a$.fxdax.extentd.v1.PlaceStoreResponse\x12W\n" +
	"\n" +
	"MakePublic\x12#.fxdax.extentd.v1.MakePublicRequest\x1a$.fxdax.extentd.v1.MakePublicResponse\x12W\n" +
	"\n" +
	"SharePoint\x12#.fxdax.extentd.v1.SharePointRequest\x1a$.fxdax.extentd.v1.SharePointResponse\x12W\n" +
	"\n" +
	"AttachBase\x12#.fxdax.extentd.v1.AttachBaseRequest\x1a$.fxdax.extentd.v1.AttachBaseResponse\x12T\n" +
	"\tSyncStore\x12\".fxdax.extentd.v1.SyncStoreRequest\x1a#.fxdax.extentd.v1.SyncStoreResponse\x12T\n" +
	"\tStatStore\x12\".fxdax.extentd.v1.StatStoreRequest\x1a#.fxdax.extentd.v1.StatStoreResponse\x12W\n" +
	"\n" +
	"ListStores\x12#.fxdax.extentd.v1.ListStoresRequest\x1a$.fxdax.extentd.v1.ListStoresResponse\x12Q\n" +
	"\bGetStats\x12!.fxdax.extentd.v1.GetStatsRequest\x1a\".fxdax.extentd.v1.GetStatsResponse\x12Z\n" +
	"\vGetSnapshot\x12$.fxdax.extentd.v1.GetSnapshotRequest\x1a%.fxdax.extentd.v1.GetSnapshotResponse\x12H\n" +
	"\x05Grant\x12\x1e.fxdax.extentd.v1.GrantRequest\x1a\x1f.fxdax.extentd.v1.GrantResponse\x12K\n" +
	"\x06Revoke\x12\x1f.fxdax.extentd.v1.RevokeRequest\x1a .fxdax.extentd.v1.RevokeResponse\x12c\n" +
	"\x0eSetMemoryLimit\x12'.fxdax.extentd.v1.SetMemoryLimitRequest\x1a(.fxdax.extentd.v1.SetMemoryLimitResponse\x12f\n" +
	"\x0fSetMemoryWeight\x12(.fxdax.extentd.v1.SetMemoryWeightRequest\x1a).fxdax.extentd.v1.SetMemoryWeightResponse\x12`\n" +
	"\rSetTenantRoom\x12&.fxdax.extentd.v1.SetTenantRoomRequest\x1a'.fxdax.extentd.v1.SetTenantRoomResponse\x12]\n" +
	"\fShrinkMemory\x12%.fxdax.extentd.v1.ShrinkMemoryRequest\x1a&.fxdax.extentd.v1.ShrinkMemoryResponse\x12]\n" +
	"\fSetDiskLimit\x12%.fxdax.extentd.v1.SetDiskLimitRequest\x1a&.fxdax.extentd.v1.SetDiskLimitResponse\x12`\n" +
	"\rGetTenantDisk\x12&.fxdax.extentd.v1.GetTenantDiskRequest\x1a'.fxdax.extentd.v1.GetTenantDiskResponseB5Z3github.com/semistrict/fxdax/go/extentd/v1;extentdv1b\x06proto3"

var (
	file_fxdax_extentd_v1_extentd_proto_rawDescOnce sync.Once
	file_fxdax_extentd_v1_extentd_proto_rawDescData []byte
)

func file_fxdax_extentd_v1_extentd_proto_rawDescGZIP() []byte {
	file_fxdax_extentd_v1_extentd_proto_rawDescOnce.Do(func() {
		file_fxdax_extentd_v1_extentd_proto_rawDescData = protoimpl.X.CompressGZIP(unsafe.Slice(unsafe.StringData(file_fxdax_extentd_v1_extentd_proto_rawDesc), len(file_fxdax_extentd_v1_extentd_proto_rawDesc)))
	})
	return file_fxdax_extentd_v1_extentd_proto_rawDescData
}

var file_fxdax_extentd_v1_extentd_proto_enumTypes = make([]protoimpl.EnumInfo, 1)
var file_fxdax_extentd_v1_extentd_proto_msgTypes = make([]protoimpl.MessageInfo, 61)
var file_fxdax_extentd_v1_extentd_proto_goTypes = []any{
	(VolumeKind)(0),                 // 0: fxdax.extentd.v1.VolumeKind
	(*CreateStoreRequest)(nil),      // 1: fxdax.extentd.v1.CreateStoreRequest
	(*CreateStoreResponse)(nil),     // 2: fxdax.extentd.v1.CreateStoreResponse
	(*ImportImageRequest)(nil),      // 3: fxdax.extentd.v1.ImportImageRequest
	(*ImportImageResponse)(nil),     // 4: fxdax.extentd.v1.ImportImageResponse
	(*ForkStoreRequest)(nil),        // 5: fxdax.extentd.v1.ForkStoreRequest
	(*ForkStoreResponse)(nil),       // 6: fxdax.extentd.v1.ForkStoreResponse
	(*DeleteStoreRequest)(nil),      // 7: fxdax.extentd.v1.DeleteStoreRequest
	(*DeleteStoreResponse)(nil),     // 8: fxdax.extentd.v1.DeleteStoreResponse
	(*DropStoreRequest)(nil),        // 9: fxdax.extentd.v1.DropStoreRequest
	(*DropStoreResponse)(nil),       // 10: fxdax.extentd.v1.DropStoreResponse
	(*ResizeStoreRequest)(nil),      // 11: fxdax.extentd.v1.ResizeStoreRequest
	(*ResizeStoreResponse)(nil),     // 12: fxdax.extentd.v1.ResizeStoreResponse
	(*PlaceStoreRequest)(nil),       // 13: fxdax.extentd.v1.PlaceStoreRequest
	(*PlaceStoreResponse)(nil),      // 14: fxdax.extentd.v1.PlaceStoreResponse
	(*SharePointRequest)(nil),       // 15: fxdax.extentd.v1.SharePointRequest
	(*SharePointResponse)(nil),      // 16: fxdax.extentd.v1.SharePointResponse
	(*MakePublicRequest)(nil),       // 17: fxdax.extentd.v1.MakePublicRequest
	(*MakePublicResponse)(nil),      // 18: fxdax.extentd.v1.MakePublicResponse
	(*AttachBaseRequest)(nil),       // 19: fxdax.extentd.v1.AttachBaseRequest
	(*AttachBaseResponse)(nil),      // 20: fxdax.extentd.v1.AttachBaseResponse
	(*SyncStoreRequest)(nil),        // 21: fxdax.extentd.v1.SyncStoreRequest
	(*SyncStoreResponse)(nil),       // 22: fxdax.extentd.v1.SyncStoreResponse
	(*StatStoreRequest)(nil),        // 23: fxdax.extentd.v1.StatStoreRequest
	(*StatStoreResponse)(nil),       // 24: fxdax.extentd.v1.StatStoreResponse
	(*ListStoresRequest)(nil),       // 25: fxdax.extentd.v1.ListStoresRequest
	(*ListStoresResponse)(nil),      // 26: fxdax.extentd.v1.ListStoresResponse
	(*ListedStore)(nil),             // 27: fxdax.extentd.v1.ListedStore
	(*GetStatsRequest)(nil),         // 28: fxdax.extentd.v1.GetStatsRequest
	(*GetStatsResponse)(nil),        // 29: fxdax.extentd.v1.GetStatsResponse
	(*GetSnapshotRequest)(nil),      // 30: fxdax.extentd.v1.GetSnapshotRequest
	(*GetSnapshotResponse)(nil),     // 31: fxdax.extentd.v1.GetSnapshotResponse
	(*VmSnapshot)(nil),              // 32: fxdax.extentd.v1.VmSnapshot
	(*GuestSnapshot)(nil),           // 33: fxdax.extentd.v1.GuestSnapshot
	(*Histogram)(nil),               // 34: fxdax.extentd.v1.Histogram
	(*ObjectStorageStats)(nil),      // 35: fxdax.extentd.v1.ObjectStorageStats
	(*GrantRequest)(nil),            // 36: fxdax.extentd.v1.GrantRequest
	(*GrantResponse)(nil),           // 37: fxdax.extentd.v1.GrantResponse
	(*RevokeRequest)(nil),           // 38: fxdax.extentd.v1.RevokeRequest
	(*RevokeResponse)(nil),          // 39: fxdax.extentd.v1.RevokeResponse
	(*SetMemoryLimitRequest)(nil),   // 40: fxdax.extentd.v1.SetMemoryLimitRequest
	(*SetMemoryLimitResponse)(nil),  // 41: fxdax.extentd.v1.SetMemoryLimitResponse
	(*SetMemoryWeightRequest)(nil),  // 42: fxdax.extentd.v1.SetMemoryWeightRequest
	(*SetMemoryWeightResponse)(nil), // 43: fxdax.extentd.v1.SetMemoryWeightResponse
	(*SetTenantRoomRequest)(nil),    // 44: fxdax.extentd.v1.SetTenantRoomRequest
	(*SetTenantRoomResponse)(nil),   // 45: fxdax.extentd.v1.SetTenantRoomResponse
	(*ShrinkMemoryRequest)(nil),     // 46: fxdax.extentd.v1.ShrinkMemoryRequest
	(*ShrinkMemoryResponse)(nil),    // 47: fxdax.extentd.v1.ShrinkMemoryResponse
	(*SetDiskLimitRequest)(nil),     // 48: fxdax.extentd.v1.SetDiskLimitRequest
	(*SetDiskLimitResponse)(nil),    // 49: fxdax.extentd.v1.SetDiskLimitResponse
	(*GetTenantDiskRequest)(nil),    // 50: fxdax.extentd.v1.GetTenantDiskRequest
	(*GetTenantDiskResponse)(nil),   // 51: fxdax.extentd.v1.GetTenantDiskResponse
	nil,                             // 52: fxdax.extentd.v1.GetSnapshotResponse.CountersEntry
	nil,                             // 53: fxdax.extentd.v1.GetSnapshotResponse.GaugesEntry
	nil,                             // 54: fxdax.extentd.v1.VmSnapshot.CountersEntry
	nil,                             // 55: fxdax.extentd.v1.VmSnapshot.GaugesEntry
	nil,                             // 56: fxdax.extentd.v1.VmSnapshot.WindowEntry
	nil,                             // 57: fxdax.extentd.v1.GuestSnapshot.CountersEntry
	nil,                             // 58: fxdax.extentd.v1.GuestSnapshot.HistogramsEntry
	nil,                             // 59: fxdax.extentd.v1.ObjectStorageStats.RequestsEntry
	nil,                             // 60: fxdax.extentd.v1.ObjectStorageStats.UploadedBytesEntry
	nil,                             // 61: fxdax.extentd.v1.ObjectStorageStats.FailuresEntry
}
var file_fxdax_extentd_v1_extentd_proto_depIdxs = []int32{
	0,  // 0: fxdax.extentd.v1.CreateStoreRequest.kind:type_name -> fxdax.extentd.v1.VolumeKind
	27, // 1: fxdax.extentd.v1.ListStoresResponse.stores:type_name -> fxdax.extentd.v1.ListedStore
	24, // 2: fxdax.extentd.v1.ListedStore.stat:type_name -> fxdax.extentd.v1.StatStoreResponse
	35, // 3: fxdax.extentd.v1.GetStatsResponse.object_storage:type_name -> fxdax.extentd.v1.ObjectStorageStats
	52, // 4: fxdax.extentd.v1.GetSnapshotResponse.counters:type_name -> fxdax.extentd.v1.GetSnapshotResponse.CountersEntry
	53, // 5: fxdax.extentd.v1.GetSnapshotResponse.gauges:type_name -> fxdax.extentd.v1.GetSnapshotResponse.GaugesEntry
	32, // 6: fxdax.extentd.v1.GetSnapshotResponse.vms:type_name -> fxdax.extentd.v1.VmSnapshot
	54, // 7: fxdax.extentd.v1.VmSnapshot.counters:type_name -> fxdax.extentd.v1.VmSnapshot.CountersEntry
	55, // 8: fxdax.extentd.v1.VmSnapshot.gauges:type_name -> fxdax.extentd.v1.VmSnapshot.GaugesEntry
	56, // 9: fxdax.extentd.v1.VmSnapshot.window:type_name -> fxdax.extentd.v1.VmSnapshot.WindowEntry
	33, // 10: fxdax.extentd.v1.VmSnapshot.guest:type_name -> fxdax.extentd.v1.GuestSnapshot
	57, // 11: fxdax.extentd.v1.GuestSnapshot.counters:type_name -> fxdax.extentd.v1.GuestSnapshot.CountersEntry
	58, // 12: fxdax.extentd.v1.GuestSnapshot.histograms:type_name -> fxdax.extentd.v1.GuestSnapshot.HistogramsEntry
	59, // 13: fxdax.extentd.v1.ObjectStorageStats.requests:type_name -> fxdax.extentd.v1.ObjectStorageStats.RequestsEntry
	60, // 14: fxdax.extentd.v1.ObjectStorageStats.uploaded_bytes:type_name -> fxdax.extentd.v1.ObjectStorageStats.UploadedBytesEntry
	61, // 15: fxdax.extentd.v1.ObjectStorageStats.failures:type_name -> fxdax.extentd.v1.ObjectStorageStats.FailuresEntry
	34, // 16: fxdax.extentd.v1.GuestSnapshot.HistogramsEntry.value:type_name -> fxdax.extentd.v1.Histogram
	1,  // 17: fxdax.extentd.v1.Extentd.CreateStore:input_type -> fxdax.extentd.v1.CreateStoreRequest
	3,  // 18: fxdax.extentd.v1.Extentd.ImportImage:input_type -> fxdax.extentd.v1.ImportImageRequest
	5,  // 19: fxdax.extentd.v1.Extentd.ForkStore:input_type -> fxdax.extentd.v1.ForkStoreRequest
	7,  // 20: fxdax.extentd.v1.Extentd.DeleteStore:input_type -> fxdax.extentd.v1.DeleteStoreRequest
	9,  // 21: fxdax.extentd.v1.Extentd.DropStore:input_type -> fxdax.extentd.v1.DropStoreRequest
	11, // 22: fxdax.extentd.v1.Extentd.ResizeStore:input_type -> fxdax.extentd.v1.ResizeStoreRequest
	13, // 23: fxdax.extentd.v1.Extentd.PlaceStore:input_type -> fxdax.extentd.v1.PlaceStoreRequest
	17, // 24: fxdax.extentd.v1.Extentd.MakePublic:input_type -> fxdax.extentd.v1.MakePublicRequest
	15, // 25: fxdax.extentd.v1.Extentd.SharePoint:input_type -> fxdax.extentd.v1.SharePointRequest
	19, // 26: fxdax.extentd.v1.Extentd.AttachBase:input_type -> fxdax.extentd.v1.AttachBaseRequest
	21, // 27: fxdax.extentd.v1.Extentd.SyncStore:input_type -> fxdax.extentd.v1.SyncStoreRequest
	23, // 28: fxdax.extentd.v1.Extentd.StatStore:input_type -> fxdax.extentd.v1.StatStoreRequest
	25, // 29: fxdax.extentd.v1.Extentd.ListStores:input_type -> fxdax.extentd.v1.ListStoresRequest
	28, // 30: fxdax.extentd.v1.Extentd.GetStats:input_type -> fxdax.extentd.v1.GetStatsRequest
	30, // 31: fxdax.extentd.v1.Extentd.GetSnapshot:input_type -> fxdax.extentd.v1.GetSnapshotRequest
	36, // 32: fxdax.extentd.v1.Extentd.Grant:input_type -> fxdax.extentd.v1.GrantRequest
	38, // 33: fxdax.extentd.v1.Extentd.Revoke:input_type -> fxdax.extentd.v1.RevokeRequest
	40, // 34: fxdax.extentd.v1.Extentd.SetMemoryLimit:input_type -> fxdax.extentd.v1.SetMemoryLimitRequest
	42, // 35: fxdax.extentd.v1.Extentd.SetMemoryWeight:input_type -> fxdax.extentd.v1.SetMemoryWeightRequest
	44, // 36: fxdax.extentd.v1.Extentd.SetTenantRoom:input_type -> fxdax.extentd.v1.SetTenantRoomRequest
	46, // 37: fxdax.extentd.v1.Extentd.ShrinkMemory:input_type -> fxdax.extentd.v1.ShrinkMemoryRequest
	48, // 38: fxdax.extentd.v1.Extentd.SetDiskLimit:input_type -> fxdax.extentd.v1.SetDiskLimitRequest
	50, // 39: fxdax.extentd.v1.Extentd.GetTenantDisk:input_type -> fxdax.extentd.v1.GetTenantDiskRequest
	2,  // 40: fxdax.extentd.v1.Extentd.CreateStore:output_type -> fxdax.extentd.v1.CreateStoreResponse
	4,  // 41: fxdax.extentd.v1.Extentd.ImportImage:output_type -> fxdax.extentd.v1.ImportImageResponse
	6,  // 42: fxdax.extentd.v1.Extentd.ForkStore:output_type -> fxdax.extentd.v1.ForkStoreResponse
	8,  // 43: fxdax.extentd.v1.Extentd.DeleteStore:output_type -> fxdax.extentd.v1.DeleteStoreResponse
	10, // 44: fxdax.extentd.v1.Extentd.DropStore:output_type -> fxdax.extentd.v1.DropStoreResponse
	12, // 45: fxdax.extentd.v1.Extentd.ResizeStore:output_type -> fxdax.extentd.v1.ResizeStoreResponse
	14, // 46: fxdax.extentd.v1.Extentd.PlaceStore:output_type -> fxdax.extentd.v1.PlaceStoreResponse
	18, // 47: fxdax.extentd.v1.Extentd.MakePublic:output_type -> fxdax.extentd.v1.MakePublicResponse
	16, // 48: fxdax.extentd.v1.Extentd.SharePoint:output_type -> fxdax.extentd.v1.SharePointResponse
	20, // 49: fxdax.extentd.v1.Extentd.AttachBase:output_type -> fxdax.extentd.v1.AttachBaseResponse
	22, // 50: fxdax.extentd.v1.Extentd.SyncStore:output_type -> fxdax.extentd.v1.SyncStoreResponse
	24, // 51: fxdax.extentd.v1.Extentd.StatStore:output_type -> fxdax.extentd.v1.StatStoreResponse
	26, // 52: fxdax.extentd.v1.Extentd.ListStores:output_type -> fxdax.extentd.v1.ListStoresResponse
	29, // 53: fxdax.extentd.v1.Extentd.GetStats:output_type -> fxdax.extentd.v1.GetStatsResponse
	31, // 54: fxdax.extentd.v1.Extentd.GetSnapshot:output_type -> fxdax.extentd.v1.GetSnapshotResponse
	37, // 55: fxdax.extentd.v1.Extentd.Grant:output_type -> fxdax.extentd.v1.GrantResponse
	39, // 56: fxdax.extentd.v1.Extentd.Revoke:output_type -> fxdax.extentd.v1.RevokeResponse
	41, // 57: fxdax.extentd.v1.Extentd.SetMemoryLimit:output_type -> fxdax.extentd.v1.SetMemoryLimitResponse
	43, // 58: fxdax.extentd.v1.Extentd.SetMemoryWeight:output_type -> fxdax.extentd.v1.SetMemoryWeightResponse
	45, // 59: fxdax.extentd.v1.Extentd.SetTenantRoom:output_type -> fxdax.extentd.v1.SetTenantRoomResponse
	47, // 60: fxdax.extentd.v1.Extentd.ShrinkMemory:output_type -> fxdax.extentd.v1.ShrinkMemoryResponse
	49, // 61: fxdax.extentd.v1.Extentd.SetDiskLimit:output_type -> fxdax.extentd.v1.SetDiskLimitResponse
	51, // 62: fxdax.extentd.v1.Extentd.GetTenantDisk:output_type -> fxdax.extentd.v1.GetTenantDiskResponse
	40, // [40:63] is the sub-list for method output_type
	17, // [17:40] is the sub-list for method input_type
	17, // [17:17] is the sub-list for extension type_name
	17, // [17:17] is the sub-list for extension extendee
	0,  // [0:17] is the sub-list for field type_name
}

func init() { file_fxdax_extentd_v1_extentd_proto_init() }
func file_fxdax_extentd_v1_extentd_proto_init() {
	if File_fxdax_extentd_v1_extentd_proto != nil {
		return
	}
	file_fxdax_extentd_v1_extentd_proto_msgTypes[0].OneofWrappers = []any{}
	file_fxdax_extentd_v1_extentd_proto_msgTypes[28].OneofWrappers = []any{}
	file_fxdax_extentd_v1_extentd_proto_msgTypes[31].OneofWrappers = []any{}
	file_fxdax_extentd_v1_extentd_proto_msgTypes[35].OneofWrappers = []any{}
	file_fxdax_extentd_v1_extentd_proto_msgTypes[39].OneofWrappers = []any{}
	file_fxdax_extentd_v1_extentd_proto_msgTypes[43].OneofWrappers = []any{}
	file_fxdax_extentd_v1_extentd_proto_msgTypes[47].OneofWrappers = []any{}
	file_fxdax_extentd_v1_extentd_proto_msgTypes[50].OneofWrappers = []any{}
	type x struct{}
	out := protoimpl.TypeBuilder{
		File: protoimpl.DescBuilder{
			GoPackagePath: reflect.TypeOf(x{}).PkgPath(),
			RawDescriptor: unsafe.Slice(unsafe.StringData(file_fxdax_extentd_v1_extentd_proto_rawDesc), len(file_fxdax_extentd_v1_extentd_proto_rawDesc)),
			NumEnums:      1,
			NumMessages:   61,
			NumExtensions: 0,
			NumServices:   1,
		},
		GoTypes:           file_fxdax_extentd_v1_extentd_proto_goTypes,
		DependencyIndexes: file_fxdax_extentd_v1_extentd_proto_depIdxs,
		EnumInfos:         file_fxdax_extentd_v1_extentd_proto_enumTypes,
		MessageInfos:      file_fxdax_extentd_v1_extentd_proto_msgTypes,
	}.Build()
	File_fxdax_extentd_v1_extentd_proto = out.File
	file_fxdax_extentd_v1_extentd_proto_goTypes = nil
	file_fxdax_extentd_v1_extentd_proto_depIdxs = nil
}
