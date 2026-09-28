# fxdax

Fuchsia's Fxfs filesystem, running outside Fuchsia so that VMs can use it over
virtio-fs with DAX, the way Fuchsia programs use it through Zircon pagers.

## Layout

- `vendor/fuchsia/`: Fuchsia sources, identical to upstream at the commit in
  `vendor/fuchsia/COMMIT`. `scripts/vendor-fuchsia.sh` copies them.
- `crates/`: one Cargo package per vendored crate, pointing at its sources,
  plus stand-ins for what only exists inside Fuchsia's build:
  - `fidl-fuchsia-*`: the generated FIDL binding types Fxfs uses
  - `fuchsia-inspect`: diagnostics, as no-ops
  - `diagnostics-log`: logging to stderr
  - `bssl-crypto`, `mundane`: SHA-256/512 on the `sha2` crate
  - `fxfs-unicode-gen`: Unicode tables from UCD 12.1.0, made by Fxfs's own
    generator (`scripts/generate-unicode-tables.sh`)

## Testing

```sh
scripts/test.sh          # macOS or Linux
scripts/test-linux.sh    # Linux, in a Lima VM
scripts/test-gce.sh      # x86-64 Linux with KVM, on a disposable GCE host
```

The guest tests boot a libkrun guest with an fxdax share, with DAX and
without, and check the image on the host afterwards:

```sh
scripts/test-e2e.sh        # macOS (HVF) or Linux (KVM)
scripts/test-e2e-linux.sh  # Linux, in a Lima VM
```

`scripts/test-gce.sh` runs them too, and copies the logs back. On macOS they
need Homebrew's `llvm` and `libkrunfw`.

On Linux, DAX windows take KVM's faults through userfaultfd, which needs
access to `/dev/userfaultfd` (or `vm.unprivileged_userfaultfd=1`). To grant it
to the `kvm` group:

```sh
echo 'KERNEL=="userfaultfd", GROUP="kvm", MODE="0660"' |
    sudo tee /etc/udev/rules.d/99-userfaultfd.rules
sudo udevadm control --reload && sudo udevadm trigger --name-match=userfaultfd
```
