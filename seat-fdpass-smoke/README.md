# seat-fdpass-smoke

A throwaway VM test that validates the load-bearing claim behind rev's seat
layer (`rev/src/seat`): that a root process can open a restricted DRM node and
hand the file descriptor to an *unprivileged* process over a Unix socket
(SCM_RIGHTS), and that the passed fd carries enough authority for that
unprivileged process to drive KMS (modeset). This is the logind/seatd/libseat
replacement model, so it has to actually hold on real hardware, not just in
unit tests.

RunixOS is a Linux kernel plus glibc, so this is plain Linux-kernel behaviour
and proves out identically on RunixOS.

## What it does

The binary is PID 1 of a tiny initramfs. It:

1. brings up `/proc`, `/sys`, `/dev`, and loads `virtio-gpu` so
   `/dev/dri/card0` appears,
2. opens the card as root and becomes DRM master (`DRM_IOCTL_SET_MASTER`) --
   modern kernels do not auto-grant master on open,
3. forks, drops the child to an unprivileged uid (nobody),
4. CONTROL: the child tries to open the card itself and must be denied
   (proving the fd-pass is actually required),
5. passes the open fd to the child via SCM_RIGHTS, using the exact
   `send_fd`/`recv_fd` code from `rev/src/seat/fd_passing.rs`,
6. the child drives the passed fd: `DRM_IOCTL_VERSION` (real device),
   `MODE_GETRESOURCES` (KMS reachable), and a master-gated `MODE_SETCRTC`
   (proves the unprivileged peer holds master authority via the fd),
7. reports PASS/FAIL and powers off.

The master-gated `SETCRTC` is the key probe: it is allowed based on
`is_current_master` of the *open file description* (which rev, as root,
established), not on the caller's capabilities. So an unprivileged compositor
can modeset purely because it was handed the master fd. Note that the
compositor must NOT call `SET_MASTER` itself -- that is gated on the opener's
pid / CAP_SYS_ADMIN and will fail for a forked, unprivileged peer. rev becomes
master; the compositor inherits it through the fd.

## What it found

rev's `seat::open_device` originally only did `open(O_RDWR|O_CLOEXEC)` and
never `SET_MASTER`. On a kernel that does not auto-grant master (the case
here), the compositor would receive a non-master fd and every modeset would
return EACCES. Fix: rev now `SET_MASTER`s primary DRM nodes on open, while it
holds CAP_SYS_ADMIN as root, so master rides the fd to the compositor.

## Run

```sh
RUSTFLAGS="-C target-feature=+crt-static" \
  cargo build --release --target x86_64-unknown-linux-gnu
./run-vm.sh
```

Needs: qemu-system-x86_64 with KVM, a host kernel whose `virtio-gpu` module
matches `uname -r`, and zstd/cpio/gzip. The script builds the initramfs and
boots QEMU with `-device virtio-gpu-pci -cpu host -nographic` on the serial
console; the binary runs and powers the VM off on its own.
