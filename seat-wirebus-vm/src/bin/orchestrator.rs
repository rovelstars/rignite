//! PID 1 for the rev seat end-to-end VM test.
//!
//! Brings up the namespace, loads virtio-gpu / virtio_input, starts the REAL
//! rev daemon (`rev bus-serve --seat-session 1`) on a WireBus Highway socket,
//! then runs the unprivileged seat-client against it and powers off.

use std::ffi::CString;
use std::process::Command;

const BUS_SOCK: &str = "/run/rev-bus.sock";

fn main() {
    println!("\n=== seat-wirebus-vm: rev real-daemon seat test ===");

    mount("proc", "/proc", "proc");
    mount("sysfs", "/sys", "sysfs");
    mount("devtmpfs", "/dev", "devtmpfs");
    let _ = std::fs::create_dir_all("/run");

    load_module("/virtio_dma_buf.ko");
    load_module("/virtio-gpu.ko");
    load_module("/virtio_input.ko");
    wait_for("/dev/dri/card0", 120);

    std::env::set_var("REV_BUS_SOCK", BUS_SOCK);

    // The real rev daemon, serving only the Highway, with an active seat session
    // so a root (System) client may OpenDevice.
    let mut rev = match Command::new("/rev")
        .args(["bus-serve", "--seat-session", "1"])
        .spawn()
    {
        Ok(c) => c,
        Err(e) => fail(&format!("spawn /rev: {e}")),
    };
    println!("[orch] started rev daemon (pid {})", rev.id());

    if !wait_for(BUS_SOCK, 200) {
        let _ = rev.kill();
        fail("rev never created the bus socket");
    }
    println!("[orch] bus socket up at {BUS_SOCK}");

    let status = Command::new("/seat-client").status();
    let code = match status {
        Ok(s) => s.code().unwrap_or(-1),
        Err(e) => {
            println!("[orch] seat-client failed to run: {e}");
            -1
        }
    };

    let _ = rev.kill();
    println!("\n=== RESULT: {} (seat-client exit {code}) ===",
        if code == 0 { "PASS -- real rev daemon handed working device fds over WireBus" } else { "FAIL" });
    println!("=== seat-wirebus-vm done; powering off ===\n");
    sync_and_poweroff();
}

fn mount(src: &str, target: &str, fstype: &str) {
    let _ = std::fs::create_dir_all(target);
    let (s, t, f) = (CString::new(src).unwrap(), CString::new(target).unwrap(), CString::new(fstype).unwrap());
    if unsafe { libc::mount(s.as_ptr(), t.as_ptr(), f.as_ptr(), 0, std::ptr::null()) } != 0 {
        println!("[!] mount {target} failed: {}", std::io::Error::last_os_error());
    }
}

fn load_module(path: &str) {
    let c = CString::new(path).unwrap();
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        println!("[!] open module {path}: {}", std::io::Error::last_os_error());
        return;
    }
    let params = CString::new("").unwrap();
    let r = unsafe { libc::syscall(libc::SYS_finit_module, fd, params.as_ptr(), 0) };
    unsafe { libc::close(fd) };
    if r != 0 {
        println!("[!] finit_module {path}: {} (continuing)", std::io::Error::last_os_error());
    }
}

fn wait_for(path: &str, tries: u32) -> bool {
    for _ in 0..tries {
        if std::path::Path::new(path).exists() {
            return true;
        }
        unsafe { libc::usleep(25_000) };
    }
    false
}

fn sync_and_poweroff() -> ! {
    unsafe {
        libc::sync();
        libc::reboot(libc::RB_POWER_OFF);
    }
    loop {
        unsafe { libc::pause() };
    }
}

fn fail(msg: &str) -> ! {
    println!("\n[FATAL] {msg}");
    println!("=== RESULT: FAIL (setup) ===");
    sync_and_poweroff();
}
