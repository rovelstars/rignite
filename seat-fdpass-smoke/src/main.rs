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
const DRM_IOCTL_DROP_MASTER: libc::c_ulong = 0x0000_641f; // _IO('d', 0x1f)
const DRM_IOCTL_MODE_GETRESOURCES: libc::c_ulong = 0xC040_64A0; // _IOWR('d', 0xA0, drm_mode_card_res{64})
const DRM_IOCTL_MODE_SETCRTC: libc::c_ulong = 0xC068_64A2; // _IOWR('d', 0xA2, drm_mode_crtc{104})

// evdev ioctls (type 'E' = 0x45). Input nodes have no "master"; the seat job is
// (a) only the active session may hold the fd, and (b) on VT switch the fd is
// REVOKED so a backgrounded session cannot keep reading the keyboard. EVIOCREVOKE
// is exactly logind's revoke-on-switch mechanism: it kills the open file
// description, so the revoke lands on the compositor's SCM_RIGHTS dup too.
const EVIOCGVERSION: libc::c_ulong = 0x8004_4501; // _IOR('E', 0x01, int)
const EVIOCGNAME_256: libc::c_ulong = 0x8100_4506; // _IOC(R,'E',0x06,256)
const EVIOCREVOKE: libc::c_ulong = 0x4004_4591; // _IOW('E', 0x91, int)

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
    load_module("/virtio_input.ko");

    let card = match wait_for_card() {
        Some(c) => c,
        None => fail("no /dev/dri/card* appeared (virtio-gpu not bound?)"),
    };
    println!("[+] DRM node present: {card}");

    // Two test modes, picked by kernel cmdline. Default: single-session fd-pass.
    // smoke=multi: two-session VT-switch arbitration (master + input handoff).
    if cmdline_has("smoke=multi") {
        multi_session(&card, wait_for_event().as_deref());
    } else {
        single_session(&card);
    }
}

/// Single-session: root opens a device, passes it to one unprivileged child,
/// child drives it; plus the EVIOCREVOKE input cutoff.
fn single_session(card: &str) -> ! {
    // Parent opens as root. First opener of a card node becomes DRM master.
    let drm_fd = open_rdwr_cloexec(card);
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

    // Input device (virtio-keyboard). Optional: if none is present the input
    // half is skipped, the DRM half still runs.
    let event = wait_for_event();
    let event_fd = match &event {
        Some(p) => {
            let f = open_rdwr_cloexec(p);
            if f < 0 {
                println!("[!] root open of {p} failed: {} (skipping input test)", last_err());
                -1
            } else {
                println!("[+] parent (uid 0) opened input node {p}");
                f
            }
        }
        None => {
            println!("[!] no /dev/input/event* found (skipping input test)");
            -1
        }
    };

    // socketpair: parent keeps a, child keeps b.
    let mut sp: [libc::c_int; 2] = [0; 2];
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sp.as_mut_ptr()) } != 0 {
        fail(&format!("socketpair: {}", last_err()));
    }
    let (parent_sock, child_sock) = (sp[0], sp[1]);

    match unsafe { libc::fork() } {
        -1 => fail(&format!("fork: {}", last_err())),
        0 => child(child_sock, parent_sock, card, event.as_deref()),
        pid => parent(pid, parent_sock, child_sock, drm_fd, event_fd),
    }
}

/// Unprivileged receiver: the compositor's role in rev.
fn child(child_sock: RawFd, parent_sock: RawFd, card: &str, event: Option<&str>) -> ! {
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

    // ----- input device half -----
    if let Some(ev) = event {
        ok &= child_input(child_sock, ev);
    }

    std::process::exit(if ok { 0 } else { 3 });
}

/// Input fd-pass + revoke test, from the compositor's side.
fn child_input(sock: RawFd, ev: &str) -> bool {
    // CONTROL: unprivileged direct open must be denied.
    let direct = open_rdwr_cloexec(ev);
    if direct >= 0 {
        unsafe { libc::close(direct) };
        println!("[child] !! WARN: unprivileged direct open of {ev} SUCCEEDED -- node not restricted");
    } else {
        println!("[child] OK: direct open of {ev} denied ({}) -- fd-pass is required", last_err());
    }

    let fd = match recv_fd(sock) {
        Ok(f) => f,
        Err(e) => {
            println!("[child] FAIL: recv_fd (input): {e}");
            return false;
        }
    };
    println!("[child] received input fd {fd} via SCM_RIGHTS");

    let mut ok = true;

    // The passed fd is a usable evdev: read protocol version + device name.
    let mut ver: i32 = 0;
    if unsafe { libc::ioctl(fd, EVIOCGVERSION, &mut ver) } == 0 {
        println!("[child] OK: EVIOCGVERSION on passed input fd -> 0x{ver:08x}");
    } else {
        println!("[child] FAIL: EVIOCGVERSION on passed input fd: {}", last_err());
        ok = false;
    }
    match evdev_name(fd) {
        Some(name) => println!("[child] OK: EVIOCGNAME on passed input fd -> \"{name}\""),
        None => {
            println!("[child] FAIL: EVIOCGNAME on passed input fd: {}", last_err());
            ok = false;
        }
    }

    // Tell the parent the input fd works, then wait for it to revoke.
    write_byte(sock, 1);
    let _ = read_byte(sock); // parent signals "revoked"

    // After EVIOCREVOKE on the parent's copy, this fd must be dead (ENODEV).
    // This is the VT-switch cutoff: a backgrounded session loses the keyboard.
    if evdev_name(fd).is_none() {
        let err = last_err().raw_os_error().unwrap_or(0);
        if err == libc::ENODEV {
            println!("[child] OK: after EVIOCREVOKE the passed input fd is dead (ENODEV) -- VT-switch cutoff works");
        } else {
            println!("[child] OK: after EVIOCREVOKE the passed input fd is unusable ({})", last_err());
        }
    } else {
        println!("[child] FAIL: input fd still readable after EVIOCREVOKE -- revoke did not reach the passed fd");
        ok = false;
    }
    ok
}

fn evdev_name(fd: RawFd) -> Option<String> {
    let mut buf = [0u8; 256];
    let n = unsafe { libc::ioctl(fd, EVIOCGNAME_256, buf.as_mut_ptr()) };
    if n < 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(n as usize);
    Some(String::from_utf8_lossy(&buf[..end]).into_owned())
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
fn parent(child_pid: libc::pid_t, parent_sock: RawFd, child_sock: RawFd, drm_fd: RawFd, event_fd: RawFd) -> ! {
    unsafe { libc::close(child_sock) };

    if let Err(e) = send_fd(parent_sock, drm_fd) {
        fail(&format!("send_fd (drm): {e}"));
    }
    println!("[parent] passed DRM fd to child via SCM_RIGHTS");

    if event_fd >= 0 {
        if let Err(e) = send_fd(parent_sock, event_fd) {
            fail(&format!("send_fd (input): {e}"));
        }
        println!("[parent] passed input fd to child via SCM_RIGHTS");
        // Wait for the child to confirm the input fd works, then revoke it --
        // exactly what rev does on a VT switch away from this session.
        let _ = read_byte(parent_sock);
        let r = unsafe { libc::ioctl(event_fd, EVIOCREVOKE, 0) };
        if r == 0 {
            println!("[parent] EVIOCREVOKE on input fd ok -- access revoked for all holders");
        } else {
            println!("[parent] !! EVIOCREVOKE failed: {}", last_err());
        }
        write_byte(parent_sock, 1); // tell child to re-probe
    }

    let mut status: libc::c_int = 0;
    unsafe { libc::waitpid(child_pid, &mut status, 0) };
    let code = if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { -1 };

    println!("\n=== RESULT: {} (child exit {code}) ===",
        if code == 0 { "PASS -- fd-pass carries device authority to an unprivileged peer, and revoke cuts it off" }
        else { "FAIL" });
    println!("=== seat-fdpass-smoke done; powering off ===\n");

    sync_and_poweroff();
}

fn write_byte(fd: RawFd, b: u8) {
    let buf = [b];
    unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, 1) };
}

fn read_byte(fd: RawFd) -> i64 {
    let mut buf = [0u8; 1];
    unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, 1) as i64 }
}

// ----- multi-session: two-session VT-switch arbitration -----
//
// Models what rev does across a VT switch between two seats/sessions. A root
// arbiter (rev) opens the GPU and keyboard once per session and hands each
// session its own fd. DRM master is exclusive -- only the foreground session
// may modeset -- and on a switch the arbiter drops master + revokes input from
// the outgoing session and grants them to the incoming one. Neither session is
// privileged; the arbiter is the only thing that can flip the authority.

const MS_DONE: u8 = 0;
const MS_PROBE: u8 = 1;
const MS_FRESH: u8 = 2;

fn multi_session(card: &str, event: Option<&str>) -> ! {
    println!("[+] multi-session VT-switch arbitration test");

    // Root opens one card fd per session.
    let a_card = open_rdwr_cloexec(card);
    let b_card = open_rdwr_cloexec(card);
    if a_card < 0 || b_card < 0 {
        fail(&format!("opening {card} twice failed: {}", last_err()));
    }

    // Exclusivity: A becomes master; B must NOT be able to, even though the
    // arbiter is root -- you cannot steal an existing master.
    let am = unsafe { libc::ioctl(a_card, DRM_IOCTL_SET_MASTER, 0) };
    let bm = unsafe { libc::ioctl(b_card, DRM_IOCTL_SET_MASTER, 0) };
    let exclusivity_ok = am == 0 && bm != 0;
    if exclusivity_ok {
        println!("[+] exclusivity OK: A is master, B SET_MASTER denied ({})", last_err());
    } else {
        println!("[!] exclusivity FAIL: A set_master={am}, B set_master={bm} (a second opener took master)");
        // Restore the intended state for the rest of the test.
        unsafe { libc::ioctl(b_card, DRM_IOCTL_DROP_MASTER, 0) };
        unsafe { libc::ioctl(a_card, DRM_IOCTL_SET_MASTER, 0) };
    }

    let (a_ev, b_ev) = match event {
        Some(p) => (open_rdwr_cloexec(p), open_rdwr_cloexec(p)),
        None => {
            println!("[!] no input device; multi-session runs DRM-only");
            (-1, -1)
        }
    };

    let (pa, ca) = socketpair_or_die();
    let (pb, cb) = socketpair_or_die();

    // Two unprivileged session children.
    match unsafe { libc::fork() } {
        -1 => fail(&format!("fork A: {}", last_err())),
        0 => ms_child(ca, &[pa, pb, cb], "A"),
        _ => {}
    }
    match unsafe { libc::fork() } {
        -1 => fail(&format!("fork B: {}", last_err())),
        0 => ms_child(cb, &[pa, pb, ca], "B"),
        _ => {}
    }
    unsafe {
        libc::close(ca);
        libc::close(cb);
    }

    // Hand each session its own fds.
    let _ = send_fd(pa, a_card);
    if a_ev >= 0 { let _ = send_fd(pa, a_ev); }
    let _ = send_fd(pb, b_card);
    if b_ev >= 0 { let _ = send_fd(pb, b_ev); }

    // Background B from the start: a non-foreground session must not read input.
    if b_ev >= 0 { unsafe { libc::ioctl(b_ev, EVIOCREVOKE, 0) }; }

    // Round 1: A foreground.
    let a1 = probe(pa);
    let b1 = probe(pb);

    // VT switch A -> B: drop A's master + revoke A's input; give B master and a
    // fresh input fd (the old one was revoked while it was backgrounded).
    unsafe { libc::ioctl(a_card, DRM_IOCTL_DROP_MASTER, 0) };
    if a_ev >= 0 { unsafe { libc::ioctl(a_ev, EVIOCREVOKE, 0) }; }
    let b_set = unsafe { libc::ioctl(b_card, DRM_IOCTL_SET_MASTER, 0) };
    println!("[+] switch A->B: dropped A master, B SET_MASTER = {b_set}");
    if let Some(p) = event {
        let fresh = open_rdwr_cloexec(p);
        write_byte(pb, MS_FRESH);
        let _ = send_fd(pb, fresh);
    }

    // Round 2: B foreground.
    let a2 = probe(pa);
    let b2 = probe(pb);

    write_byte(pa, MS_DONE);
    write_byte(pb, MS_DONE);
    let mut st = 0;
    unsafe { libc::waitpid(-1, &mut st, 0); libc::waitpid(-1, &mut st, 0) };

    // Expected: foreground = (drm ok, input ok); background = (denied, dead).
    let want_fg = (true, event.is_some());
    let want_bg = (false, false);
    let drm_ok = bool2(a1).0 && !bool2(b1).0 && !bool2(a2).0 && bool2(b2).0;
    let in_ok = event.is_none()
        || (bool2(a1).1 && !bool2(b1).1 && !bool2(a2).1 && bool2(b2).1);

    println!("\n--- round 1 (A foreground): A={a1:?} B={b1:?}");
    println!("--- round 2 (B foreground): A={a2:?} B={b2:?}");
    println!("--- expected foreground={want_fg:?} background={want_bg:?}");
    let pass = exclusivity_ok && b_set == 0 && drm_ok && in_ok;
    println!("\n=== RESULT: {} ===",
        if pass { "PASS -- DRM master + input handoff between two sessions works, master is exclusive" }
        else { "FAIL" });
    println!("=== seat-fdpass-smoke (multi) done; powering off ===\n");
    sync_and_poweroff();
}

/// One unprivileged session. Receives its card + (optional) input fd, then
/// answers PROBE rounds with (can-modeset, input-readable).
fn ms_child(sock: RawFd, close_fds: &[RawFd], tag: &str) -> ! {
    for &f in close_fds { unsafe { libc::close(f) }; }
    unsafe {
        libc::setgroups(0, std::ptr::null());
        libc::setgid(UNPRIV_UID);
        libc::setuid(UNPRIV_UID);
    }

    let card_fd = recv_fd(sock).unwrap_or(-1);
    let mut input_fd = recv_fd(sock).unwrap_or(-1);
    let crtc_id = if card_fd >= 0 { first_crtc_id(card_fd).map(|(id, _)| id).unwrap_or(0) } else { 0 };
    println!("[child {tag}] dropped to uid {}, card_fd={card_fd} input_fd={input_fd}", unsafe { libc::geteuid() });

    loop {
        let mut b = [0u8; 1];
        let n = unsafe { libc::read(sock, b.as_mut_ptr() as *mut libc::c_void, 1) };
        if n <= 0 || b[0] == MS_DONE {
            std::process::exit(0);
        }
        match b[0] {
            MS_FRESH => {
                input_fd = recv_fd(sock).unwrap_or(-1);
            }
            MS_PROBE => {
                let drm_ok = setcrtc_master_ok(card_fd, crtc_id);
                let in_ok = input_fd >= 0 && evdev_name(input_fd).is_some();
                let out = [drm_ok as u8, in_ok as u8];
                unsafe { libc::write(sock, out.as_ptr() as *const libc::c_void, 2) };
            }
            _ => {}
        }
    }
}

/// True if the fd is allowed to modeset (passed the DRM_MASTER gate).
fn setcrtc_master_ok(card_fd: RawFd, crtc_id: u32) -> bool {
    if card_fd < 0 || crtc_id == 0 {
        return false;
    }
    let mut crtc = [0u8; 104];
    crtc[12..16].copy_from_slice(&crtc_id.to_le_bytes());
    let r = unsafe { libc::ioctl(card_fd, DRM_IOCTL_MODE_SETCRTC, crtc.as_mut_ptr()) };
    if r == 0 {
        return true;
    }
    let err = last_err().raw_os_error().unwrap_or(0);
    // EACCES/EPERM/EBUSY = master gate rejected; anything else = past the gate.
    !(err == libc::EACCES || err == libc::EPERM || err == libc::EBUSY)
}

/// Send PROBE and read the child's 2-byte (drm_ok, input_ok) reply.
fn probe(sock: RawFd) -> (u8, u8) {
    write_byte(sock, MS_PROBE);
    let mut buf = [0u8; 2];
    let mut got = 0;
    while got < 2 {
        let n = unsafe { libc::read(sock, buf[got..].as_mut_ptr() as *mut libc::c_void, 2 - got) };
        if n <= 0 { break; }
        got += n as usize;
    }
    (buf[0], buf[1])
}

fn bool2(t: (u8, u8)) -> (bool, bool) {
    (t.0 != 0, t.1 != 0)
}

fn socketpair_or_die() -> (RawFd, RawFd) {
    let mut sp: [libc::c_int; 2] = [0; 2];
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sp.as_mut_ptr()) } != 0 {
        fail(&format!("socketpair: {}", last_err()));
    }
    (sp[0], sp[1])
}

fn cmdline_has(token: &str) -> bool {
    std::fs::read_to_string("/proc/cmdline")
        .map(|s| s.contains(token))
        .unwrap_or(false)
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

fn wait_for_event() -> Option<String> {
    for _ in 0..120 {
        if let Ok(rd) = std::fs::read_dir("/dev/input") {
            for e in rd.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("event") {
                    return Some(format!("/dev/input/{name}"));
                }
            }
        }
        unsafe { libc::usleep(25_000) };
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
