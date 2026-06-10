# seat-wirebus-vm

End-to-end VM test of rev's REAL seat path. Where `seat-fdpass-smoke`
reimplements the arbiter to validate the kernel mechanism, this boots the
actual `rev` daemon and drives it over the actual WireBus Highway, so the code
under test is rev's own `open_device` (+ the SET_MASTER fix), bus server,
policy choke point, and the SCM_RIGHTS send in `handle_client`.

## What it does

- `orchestrator` is PID 1: mounts the namespace, loads virtio-gpu / virtio_input,
  starts `rev bus-serve --seat-session 1` on a Highway socket, runs the client,
  powers off.
- `seat-client` runs unprivileged: connects to the WireBus Highway, sends
  `OpenDevice` for `/dev/dri/card0` and an input node, receives the fds rev opens
  (as root) over SCM_RIGHTS, then uses the GPU fd to do a full dumb-buffer
  modeset and scan out a solid-colour frame -- a real frame, not just the master
  gate. It is a minimal compositor whose only device access comes from rev.

`rev bus-serve --seat-session <id>` is a dev/test seam: it marks an active seat
session so a root (System) client may OpenDevice without a full StartSession.

## Result

The real rev daemon hands a working GPU fd to an unprivileged client over
WireBus; the client scans out a 1280x800 frame and reads an input device name
through its passed fd. This is the seat layer working as a logind/seatd
replacement, proven with rev's own code.

## Run

```sh
# build the rev daemon and this crate first
( cd ../../rev && cargo build --release )
cargo build --release
./run-vm.sh
```

Needs qemu-system-x86_64 + KVM, a host kernel whose virtio modules match
`uname -r`, and zstd/cpio/gzip. rev and the client are dynamically linked; the
run script bundles their shared libs + the dynamic loader into the initramfs.
