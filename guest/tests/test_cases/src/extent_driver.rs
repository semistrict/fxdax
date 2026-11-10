// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The guest's virtio extent driver, in sysfs.

use std::fs;

const DRIVER: &str = "/sys/bus/virtio/drivers/virtio_extent";

/// The name of the one device the driver is bound to.
pub fn device() -> String {
    let entries = fs::read_dir(DRIVER).unwrap().map(|entry| entry.unwrap().file_name());
    let mut devices: Vec<String> = entries
        .map(|name| name.into_string().unwrap())
        .filter(|name| name.starts_with("virtio"))
        .collect();
    assert_eq!(devices.len(), 1, "devices bound: {devices:?}");
    devices.pop().unwrap()
}

/// Unbinds the driver from `device`, as root may at any time.
pub fn unbind(device: &str) {
    fs::write(format!("{DRIVER}/unbind"), device).unwrap();
}

/// Binds the driver to `device` again.
pub fn bind(device: &str) {
    fs::write(format!("{DRIVER}/bind"), device).unwrap();
}

/// fxdax: the bound device's attribute `name`, if the driver is bound to one: a check may have
/// unbound it.
pub fn attribute(name: &str) -> Option<String> {
    let entries = fs::read_dir(DRIVER).ok()?.filter_map(Result::ok);
    let mut devices =
        entries.filter(|entry| entry.file_name().to_string_lossy().starts_with("virtio"));
    fs::read_to_string(devices.next()?.path().join(name)).ok()
}

/// fxdax: how long the bound device has had a request in flight, all told (`busy_ns`): the guest's
/// time waiting for the host (TASK-263). From 0 at each bind.
pub fn busy() -> Option<std::time::Duration> {
    let ns = attribute("busy_ns")?;
    Some(std::time::Duration::from_nanos(ns.trim().parse().unwrap()))
}
