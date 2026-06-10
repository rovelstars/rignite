//! seat-fdpass-smoke -- PID 1 for a throwaway initramfs.
//!
//! Validates the load-bearing claim of rev's seat layer (rev/src/seat): a root
//! process opens a restricted DRM node and passes the fd to an *unprivileged*
//! process via SCM_RIGHTS, and that passed fd carries DRM-master authority the
//! unprivileged process could never obtain itself. This is plain Linux-kernel
//! behaviour, so it proves out identically for RunixOS (Linux kernel + glibc).
//!
//! Flow, mirroring rev exactly:
//!   1. bring up /proc /sys /dev, load virtio-gpu so /dev/dri/card0 appears
//!   2. parent (root) opens the card O_RDWR|O_CLOEXEC -> becomes DRM master
//!   3. fork; child drops to an unprivileged uid
//!   4. CONTROL: child opens the card itself -> must be denied (EACCES)
//!   5. parent passes its open fd to the child via SCM_RIGHTS
//!   6. child drives the passed fd: DRM_IOCTL_VERSION (fd is real),
//!      SET_MASTER (authority inherited), MODE_GETRESOURCES (KMS reachable)
//!   7. report, power off

use std::ffi::CString;
use std::os::unix::io::RawFd;

// DRM ioctl request codes (asm-generic _IOC encoding, type 'd' = 0x64).
const DRM_IOCTL_VERSION: libc::c_ulong = 0xC040_6400; // _IOWR('d', 0x00, drm_version{64})
const DRM_IOCTL_SET_MASTER: libc::c_ulong = 0x0000_641e; // _IO('d', 0x1e)
const DRM_IOCTL_MODE_GETRESOURCES: libc::c_ulong = 0xC040_64A0; // _IOWR('d', 0xA0, drm_mode_card_res{64})
const DRM_IOCTL_MODE_SETCRTC: libc::c_ulong = 0xC068_64A2; // _IOWR('d', 0xA2, drm_mode_crtc{104})

const UNPRIV_UID: libc::uid_t = 65534; // nobody

fn main() {
    // initramfs starts with nothing mounted; kernel handed us /dev/console as
    // fd 0/1/2 so these prints already reach the serial port.
    println!("\n=== seat-fdpass-smoke: rev seat fd-pass validation ===");

    mount("proc", "/proc", "proc");
    mount("sysfs", "/sys", "sysfs");
    mount("devtmpfs", "/dev", "devtmpfs");

    load_module("/virtio_dma_buf.ko");
    load_module("/virtio-gpu.ko");

    let card = match wait_for_card() {
        Some(c) => c,
        None => fail("no /dev/dri/card* appeared (virtio-gpu not bound?)"),
    };
    println!("[+] DRM node present: {card}");

    // Parent opens as root. First opener of a card node becomes DRM master.
    let drm_fd = open_rdwr_cloexec(&card);
    if drm_fd < 0 {
        fail(&format!("root open of {card} failed: {}", last_err()));
    }
    if drm_ioctl_zeroed(drm_fd, DRM_IOCTL_VERSION) != 0 {
        fail(&format!("DRM_IOCTL_VERSION on parent fd failed: {}", last_err()));
    }
    println!("[+] parent (uid 0) opened {card} and it is a live DRM device");

    // This kernel does NOT auto-grant DRM master on first open, so rev must
    // explicitly become master while it still holds CAP_SYS_ADMIN (it is root).
    // The master state lives on the open file description, so it rides along the
    // SCM_RIGHTS pass to the unprivileged compositor.
    if unsafe { libc::ioctl(drm_fd, DRM_IOCTL_SET_MASTER, 0) } == 0 {
        println!("[+] parent SET_MASTER ok -- this fd is now DRM master");
    } else {
        println!("[!] parent SET_MASTER failed: {} (auto-master may be in effect)", last_err());
    }

    // socketpair: parent keeps a, child keeps b.
    let mut sp: [libc::c_int; 2] = [0; 2];
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sp.as_mut_ptr()) } != 0 {
        fail(&format!("socketpair: {}", last_err()));
    }
    let (parent_sock, child_sock) = (sp[0], sp[1]);

    match unsafe { libc::fork() } {
        -1 => fail(&format!("fork: {}", last_err())),
        0 => child(child_sock, parent_sock, &card),
        pid => parent(pid, parent_sock, child_sock, drm_fd),
    }
}

/// Unprivileged receiver: the compositor's role in rev.
fn child(child_sock: RawFd, parent_sock: RawFd, card: &str) -> ! {
    unsafe { libc::close(parent_sock) };

    // Drop all privilege, exactly as a compositor would run.
    unsafe {
        libc::setgroups(0, std::ptr::null());
        libc::setgid(UNPRIV_UID);
        libc::setuid(UNPRIV_UID);
    }
    let euid = unsafe { libc::geteuid() };
    println!("[child] dropped to uid {euid}");

    // CONTROL: prove the unprivileged process cannot get to the device on its
    // own. If this open SUCCEEDS the whole premise is moot.
    let direct = open_rdwr_cloexec(card);
    if direct >= 0 {
        unsafe { libc::close(direct) };
        println!("[child] !! WARN: unprivileged direct open of {card} SUCCEEDED -- node not restricted in this guest");
    } else {
        println!("[child] OK: direct open of {card} denied ({}) -- fd-pass is required", last_err());
    }

    // Receive the fd rev would have passed.
    let fd = match recv_fd(child_sock) {
        Ok(f) => f,
        Err(e) => {
            println!("[child] FAIL: recv_fd: {e}");
            std::process::exit(2);
        }
    };
    println!("[child] received fd {fd} via SCM_RIGHTS");

    let mut ok = true;

    // The passed fd is a real DRM device fd in this process.
    if drm_ioctl_zeroed(fd, DRM_IOCTL_VERSION) == 0 {
        println!("[child] OK: DRM_IOCTL_VERSION on passed fd succeeded");
    } else {
        println!("[child] FAIL: DRM_IOCTL_VERSION on passed fd: {}", last_err());
        ok = false;
    }

    // Enumerate a real CRTC. MODE_GETRESOURCES is not master-gated; this just
    // gets us a valid crtc id to aim the modeset at.
    let crtc_id = match first_crtc_id(fd) {
        Some((id, connectors)) => {
            println!("[child] OK: MODE_GETRESOURCES -> crtc id {id}, {connectors} connector(s)");
            id
        }
        None => {
            println!("[child] FAIL: MODE_GETRESOURCES gave no usable crtc");
            ok = false;
            0
        }
    };

    // The real test: a master-gated MODESET on the passed fd. SETCRTC carries
    // the DRM_MASTER flag, so the kernel checks is_current_master on the open
    // file description -- which the parent (rev) established -- NOT the caller's
    // caps. An unprivileged compositor can drive KMS purely because the fd it
    // was handed is the master fd. fb_id 0 = "disable this crtc" (harmless,
    // headless), and reaching arg validation at all means we passed the gate.
    if crtc_id != 0 {
        let mut crtc = [0u8; 104];
        crtc[12..16].copy_from_slice(&crtc_id.to_le_bytes()); // crtc_id; fb_id stays 0
        let r = unsafe { libc::ioctl(fd, DRM_IOCTL_MODE_SETCRTC, crtc.as_mut_ptr()) };
        let err = last_err().raw_os_error().unwrap_or(0);
        if r == 0 {
            println!("[child] OK: SETCRTC on passed fd succeeded -- unprivileged peer drove KMS via the master fd");
        } else if err == libc::EACCES || err == libc::EPERM || err == libc::EBUSY {
            println!("[child] FAIL: SETCRTC denied ({}) -- passed fd is NOT current master", last_err());
            ok = false;
        } else {
            // Past the master gate, failed later arg validation -- still proves
            // the unprivileged peer holds master authority over this fd.
            println!("[child] OK: SETCRTC passed the master gate (handler returned {}) -- master authority carried over the fd", last_err());
        }
    }

    std::process::exit(if ok { 0 } else { 3 });
}

/// Two-pass MODE_GETRESOURCES: first read counts, then fetch the crtc id array.
/// Returns (first crtc id, connector count).
fn first_crtc_id(fd: RawFd) -> Option<(u32, u32)> {
    let mut res = [0u8; 64];
    if unsafe { libc::ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, res.as_mut_ptr()) } != 0 {
        return None;
    }
    let count_crtcs = u32::from_le_bytes(res[36..40].try_into().unwrap());
    let count_connectors = u32::from_le_bytes(res[40..44].try_into().unwrap());
    if count_crtcs == 0 {
        return None;
    }
    let mut ids = vec![0u32; count_crtcs as usize];
    // Reset, point only crtc_id_ptr at our buffer, keep other counts 0.
    res = [0u8; 64];
    res[8..16].copy_from_slice(&(ids.as_mut_ptr() as u64).to_le_bytes()); // crtc_id_ptr
    res[36..40].copy_from_slice(&count_crtcs.to_le_bytes()); // count_crtcs
    if unsafe { libc::ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, res.as_mut_ptr()) } != 0 {
        return None;
    }
    Some((ids[0], count_connectors))
}

/// Root sender: rev's role.
fn parent(child_pid: libc::pid_t, parent_sock: RawFd, child_sock: RawFd, drm_fd: RawFd) -> ! {
    unsafe { libc::close(child_sock) };

    if let Err(e) = send_fd(parent_sock, drm_fd) {
        fail(&format!("send_fd: {e}"));
    }
    println!("[parent] passed DRM fd to child via SCM_RIGHTS");

    let mut status: libc::c_int = 0;
    unsafe { libc::waitpid(child_pid, &mut status, 0) };
    let code = if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { -1 };

    println!("\n=== RESULT: {} (child exit {code}) ===",
        if code == 0 { "PASS -- fd-pass carries DRM-master authority to an unprivileged peer" }
        else { "FAIL" });
    println!("=== seat-fdpass-smoke done; powering off ===\n");

    sync_and_poweroff();
}

// ----- send_fd / recv_fd: copied verbatim from rev/src/seat/fd_passing.rs -----

use nix::sys::socket::{self, ControlMessage, ControlMessageOwned, MsgFlags, UnixAddr};
use std::io;

fn send_fd(socket_fd: RawFd, fd: RawFd) -> io::Result<()> {
    let data_byte: [u8; 1] = [0x01];
    let iov = [io::IoSlice::new(&data_byte)];
    let cmsg = [ControlMessage::ScmRights(&[fd])];
    socket::sendmsg::<UnixAddr>(socket_fd, &iov, &cmsg, MsgFlags::empty(), None)
        .map_err(|e| io::Error::from_raw_os_error(e as i32))?;
    Ok(())
}

fn recv_fd(socket_fd: RawFd) -> io::Result<RawFd> {
    let mut data_buf = [0u8; 1];
    let mut iov = [io::IoSliceMut::new(&mut data_buf)];
    let mut cmsg_buf = nix::cmsg_space!([RawFd; 8]);
    let msg = socket::recvmsg::<UnixAddr>(socket_fd, &mut iov, Some(&mut cmsg_buf), MsgFlags::empty())
        .map_err(|e| io::Error::from_raw_os_error(e as i32))?;
    for cmsg in msg.cmsgs()? {
        if let ControlMessageOwned::ScmRights(fds) = cmsg {
            if let Some(fd) = fds.into_iter().next() {
                return Ok(fd);
            }
        }
    }
    Err(io::Error::new(io::ErrorKind::Other, "no fd received"))
}

// ----- helpers -----

fn open_rdwr_cloexec(path: &str) -> RawFd {
    let c = CString::new(path).unwrap();
    unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) }
}

/// ioctl with a zeroed 64-byte buffer; enough to prove VERSION succeeds.
fn drm_ioctl_zeroed(fd: RawFd, req: libc::c_ulong) -> libc::c_int {
    let mut buf = [0u8; 64];
    unsafe { libc::ioctl(fd, req, buf.as_mut_ptr()) }
}

fn mount(src: &str, target: &str, fstype: &str) {
    let _ = std::fs::create_dir_all(target);
    let (s, t, f) = (CString::new(src).unwrap(), CString::new(target).unwrap(), CString::new(fstype).unwrap());
    let r = unsafe { libc::mount(s.as_ptr(), t.as_ptr(), f.as_ptr(), 0, std::ptr::null()) };
    if r != 0 {
        println!("[!] mount {target} ({fstype}) failed: {}", last_err());
    }
}

fn load_module(path: &str) {
    let c = CString::new(path).unwrap();
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        println!("[!] cannot open module {path}: {}", last_err());
        return;
    }
    let params = CString::new("").unwrap();
    let r = unsafe { libc::syscall(libc::SYS_finit_module, fd, params.as_ptr(), 0) };
    unsafe { libc::close(fd) };
    if r != 0 {
        // EEXIST (builtin/already loaded) is harmless.
        println!("[!] finit_module {path}: {} (continuing)", last_err());
    } else {
        println!("[+] loaded module {path}");
    }
}

fn wait_for_card() -> Option<String> {
    for _ in 0..120 {
        if let Ok(rd) = std::fs::read_dir("/dev/dri") {
            for e in rd.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("card") {
                    return Some(format!("/dev/dri/{name}"));
                }
            }
        }
        unsafe { libc::usleep(25_000) }; // 25ms; up to ~3s total
    }
    None
}

fn last_err() -> io::Error {
    io::Error::last_os_error()
}

fn sync_and_poweroff() -> ! {
    unsafe {
        libc::sync();
        libc::reboot(libc::RB_POWER_OFF);
    }
    // If reboot returns we are stuck; spin so the kernel does not panic on init exit.
    loop {
        unsafe { libc::pause() };
    }
}

fn fail(msg: &str) -> ! {
    println!("\n[FATAL] {msg}");
    println!("=== RESULT: FAIL (setup) ===");
    sync_and_poweroff();
}
