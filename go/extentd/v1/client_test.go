// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package extentdv1_test

import (
	"context"
	"errors"
	"net"
	"os"
	"path/filepath"
	"syscall"
	"testing"

	extentdv1 "github.com/semistrict/fxdax/go/extentd/v1"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/health"
	healthpb "google.golang.org/grpc/health/grpc_health_v1"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/status"
)

// fake serves the control API as the daemon answers it: Grant a socket, StatStore failing as the
// daemon fails a call, with a status and its errno, or with an errno entry that is none.
type fake struct {
	extentdv1.UnimplementedExtentdServer
}

func (fake) Grant(_ context.Context, req *extentdv1.GrantRequest) (*extentdv1.GrantResponse, error) {
	return &extentdv1.GrantResponse{Socket: "/run/tenants/" + req.Tenant + ".sock"}, nil
}

func (fake) StatStore(ctx context.Context, req *extentdv1.StatStoreRequest) (*extentdv1.StatStoreResponse, error) {
	errno := map[string]string{"missing": "2", "garbled": "ENOENT"}[req.Path]
	if err := grpc.SetTrailer(ctx, metadata.Pairs(extentdv1.ErrnoKey, errno)); err != nil {
		return nil, err
	}
	return nil, status.Error(codes.NotFound, "ENOENT")
}

// serve serves fake and the health service on a socket of its own until the test ends: the
// socket's path.
func serve(t *testing.T) string {
	// Not t.TempDir(), whose path, with the test's name, may be past a socket's 104 bytes on
	// macOS.
	dir, err := os.MkdirTemp("", "extentdv1")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(dir) })
	socket := filepath.Join(dir, "control.sock")
	listener, err := net.Listen("unix", socket)
	if err != nil {
		t.Fatal(err)
	}
	server := grpc.NewServer()
	extentdv1.RegisterExtentdServer(server, fake{})
	healthpb.RegisterHealthServer(server, health.NewServer())
	go server.Serve(listener)
	t.Cleanup(server.Stop)
	return socket
}

func client(t *testing.T, socket string) *extentdv1.Client {
	client, err := extentdv1.NewClient(socket)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { client.Close() })
	return client
}

func TestCalls(t *testing.T) {
	client := client(t, serve(t))
	granted, err := client.Grant(t.Context(), &extentdv1.GrantRequest{Tenant: "a", Dir: "/stores/a"})
	if err != nil {
		t.Fatal(err)
	}
	if granted.Socket != "/run/tenants/a.sock" {
		t.Errorf("Grant's socket = %q, want /run/tenants/a.sock", granted.Socket)
	}
	health, err := client.Health.Check(t.Context(), &healthpb.HealthCheckRequest{})
	if err != nil {
		t.Fatal(err)
	}
	if health.Status != healthpb.HealthCheckResponse_SERVING {
		t.Errorf("health = %v, want SERVING", health.Status)
	}
}

// A failed call's error is its status, and the errno the daemon failed with.
func TestErrno(t *testing.T) {
	client := client(t, serve(t))
	_, err := client.StatStore(t.Context(), &extentdv1.StatStoreRequest{Path: "missing"})
	if !errors.Is(err, syscall.ENOENT) {
		t.Errorf("errors.Is(%v, ENOENT) = false", err)
	}
	var errno syscall.Errno
	if !errors.As(err, &errno) || errno != syscall.ENOENT {
		t.Errorf("errors.As(%v) = %v, want ENOENT", err, errno)
	}
	if code := status.Code(err); code != codes.NotFound {
		t.Errorf("status.Code(%v) = %v, want NotFound", err, code)
	}
	if err.Error() != "rpc error: code = NotFound desc = ENOENT" {
		t.Errorf("err = %q, the call's status alone", err)
	}
}

// A call failed without an errno is its status alone.
func TestNoErrno(t *testing.T) {
	client := client(t, serve(t))
	_, err := client.SyncStore(t.Context(), &extentdv1.SyncStoreRequest{Path: "store"})
	var errno syscall.Errno
	if errors.As(err, &errno) {
		t.Errorf("errors.As(%v) found errno %v, want none", err, errno)
	}
	if code := status.Code(err); code != codes.Unimplemented {
		t.Errorf("status.Code(%v) = %v, want Unimplemented", err, code)
	}
}

// An errno entry that is no number is said, and no errno taken from it.
func TestGarbledErrno(t *testing.T) {
	client := client(t, serve(t))
	_, err := client.StatStore(t.Context(), &extentdv1.StatStoreRequest{Path: "garbled"})
	want := `rpc error: code = NotFound desc = ENOENT (its fxdax-errno ["ENOENT"] is no errno)`
	if err.Error() != want {
		t.Errorf("err = %q, want %q", err, want)
	}
	var errno syscall.Errno
	if errors.As(err, &errno) {
		t.Errorf("errors.As(%v) found errno %v, want none", err, errno)
	}
	if code := status.Code(err); code != codes.NotFound {
		t.Errorf("status.Code(%v) = %v, want NotFound", err, code)
	}
}
