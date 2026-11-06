// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Package extentdv1 is the Go client of fxdax's extent daemon's control API, fxdax.extentd.v1
// (crates/fxdax/proto/fxdax/extentd/v1/extentd.proto, TASK-219): its messages and gRPC stubs,
// generated from the .proto by go/generate.sh, and [NewClient], which connects to a daemon's
// control socket as fxdax's Rust client does (fxdax::extentd::control::Client).
//
// The package at a commit of fxdax is the API of the daemon built from that commit:
//
//	go get github.com/semistrict/fxdax/go@<commit>
package extentdv1

import (
	"context"
	"fmt"
	"net"
	"slices"
	"strconv"
	"syscall"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	healthpb "google.golang.org/grpc/health/grpc_health_v1"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/status"
)

// ErrnoKey is the metadata entry of a failed call's status that holds the errno the daemon failed
// with, a decimal number.
const ErrnoKey = "fxdax-errno"

// Client is a client of an extent daemon's control API, and of the standard health service
// (grpc.health.v1.Health) the control socket serves too: SERVING while the daemon accepts VM
// processes, NOT_SERVING as it starts and as it stops.
//
// A call's error carries the errno the daemon failed with, as a [syscall.Errno] that
// [errors.Is] and [errors.As] find, beside its gRPC status, which [status.FromError] finds:
//
//	_, err := client.StatStore(ctx, &extentdv1.StatStoreRequest{Path: path})
//	if errors.Is(err, syscall.ENOENT) { ... }
//
// The errno is Linux's, the daemon's host's, as the client is on that host: a client elsewhere,
// through a forwarded socket, would find Linux's number, which is its own only for the errnos
// below 35.
type Client struct {
	ExtentdClient
	Health healthpb.HealthClient
	conn   *grpc.ClientConn
}

// NewClient returns a client of the control API of the extent daemon whose control socket is at
// socket (the daemon's FXDAX_EXTENTD_GRPC). As [grpc.NewClient], it connects at its first call,
// and again whenever the connection is lost; opts are added to its own.
func NewClient(socket string, opts ...grpc.DialOption) (*Client, error) {
	// The target names no host: the dialer connects to the socket, whatever it asks for, so that
	// any path does, which a unix: URI would have to escape.
	dial := func(ctx context.Context, _ string) (net.Conn, error) {
		var dialer net.Dialer
		return dialer.DialContext(ctx, "unix", socket)
	}
	conn, err := grpc.NewClient("passthrough:///extentd", slices.Concat([]grpc.DialOption{
		grpc.WithContextDialer(dial),
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithChainUnaryInterceptor(withErrno),
	}, opts)...)
	if err != nil {
		return nil, fmt.Errorf("a client of the extent daemon's control socket at %s: %w", socket, err)
	}
	return &Client{
		ExtentdClient: NewExtentdClient(conn),
		Health:        healthpb.NewHealthClient(conn),
		conn:          conn,
	}, nil
}

// Close closes the client's connection.
func (c *Client) Close() error {
	return c.conn.Close()
}

// withErrno has a failed call's error carry the errno its status's [ErrnoKey] entry holds.
func withErrno(
	ctx context.Context,
	method string,
	req, reply any,
	cc *grpc.ClientConn,
	invoker grpc.UnaryInvoker,
	opts ...grpc.CallOption,
) error {
	var trailer metadata.MD
	err := invoker(ctx, method, req, reply, cc, append(slices.Clip(opts), grpc.Trailer(&trailer))...)
	if err == nil {
		return nil
	}
	values := trailer.Get(ErrnoKey)
	if len(values) == 0 {
		return err
	}
	errno, parseErr := strconv.ParseUint(values[0], 10, 32)
	if len(values) > 1 || parseErr != nil {
		return fmt.Errorf("%w (its %s %q is no errno)", err, ErrnoKey, values)
	}
	return &callError{err: err, errno: syscall.Errno(errno)}
}

// callError is a failed call's error, err, and the errno it failed with.
type callError struct {
	err   error
	errno syscall.Errno
}

func (e *callError) Error() string {
	return e.err.Error()
}

// Unwrap returns the call's error and its errno.
func (e *callError) Unwrap() []error {
	return []error{e.err, e.errno}
}

// GRPCStatus returns the call's status, as it is, for [status.FromError] and [status.Code].
func (e *callError) GRPCStatus() *status.Status {
	return status.Convert(e.err)
}
