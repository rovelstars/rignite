//! Unprivileged seat client: a minimal "compositor".
//!
//! Connects to the REAL rev daemon over the WireBus Highway, sends OpenDevice
//! for the GPU and keyboard, receives the fds rev opens (as root) over
//! SCM_RIGHTS, then uses the GPU fd to set a mode and scan out a solid colour --
//! a real frame, not just the master gate. This is the whole point of rev's
//! seat layer: an unprivileged process drives KMS using only fds rev handed it.

use std::io;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;

use nix::sys::socket::{self, ControlMessageOwned, MsgFlags, UnixAddr};
use wirebus_proto::{sync, Message, MessageBody};

// DRM ioctls (type 'd' = 0x64).
const DRM_IOCTL_MODE_GETRESOURCES: libc::c_ulong = 0xC040_64A0;
const DRM_IOCTL_MODE_GETCONNECTOR: libc::c_ulong = 0xC050_64A7;
const DRM_IOCTL_MODE_CREATE_DUMB: libc::c_ulong = 0xC020_64B2;
const DRM_IOCTL_MODE_ADDFB: libc::c_ulong = 0xC01C_64AE;
const DRM_IOCTL_MODE_MAP_DUMB: libc::c_ulong = 0xC010_64B3;
const DRM_IOCTL_MODE_SETCRTC: libc::c_ulong = 0xC068_64A2;
const EVIOCGNAME_256: libc::c_ulong = 0x8100_4506;

fn main() {
    println!("[client] connecting to WireBus Highway at {:?}", wirebus_proto::highway_socket());
    let mut ok = true;

    let card_fd = match open_device("/dev/dri/card0") {
        Ok(fd) => {
            println!("[client] received GPU fd {fd} from rev over WireBus");
            fd
        }
        Err(e) => {
            println!("[client] FAIL: OpenDevice card0: {e}");
            std::process::exit(2);
        }
    };

    match render_frame(card_fd) {
        Ok(msg) => println!("[client] OK: {msg}"),
        Err(e) => {
            println!("[client] FAIL: render: {e}");
            ok = false;
        }
    }

    match open_device("/dev/input/event0") {
        Ok(fd) => {
            println!("[client] received input fd {fd} from rev over WireBus");
            match evdev_name(fd) {
                Some(n) => println!("[client] OK: input device is \"{n}\""),
                None => {
                    println!("[client] FAIL: EVIOCGNAME on input fd: {}", io::Error::last_os_error());
                    ok = false;
                }
            }
        }
        Err(e) => println!("[client] note: OpenDevice event0: {e} (input optional)"),
    }

    std::process::exit(if ok { 0 } else { 3 });
}

/// One WireBus OpenDevice round-trip: send the request, read the Ok, receive the
/// fd rev passes via SCM_RIGHTS on the same connection.
fn open_device(path: &str) -> io::Result<RawFd> {
    let sock = wirebus_proto::highway_socket();
    let mut stream = UnixStream::connect(&sock)?;
    let msg = Message {
        id: 1,
        sender: "seat-client".to_string(),
        auth_token: None,
        body: MessageBody::OpenDevice { path: path.to_string() },
    };
    sync::send_message(&mut stream, &msg)?;
    let resp = sync::recv_message(&mut stream)?;
    match resp.body {
        MessageBody::Ok { .. } => {
            let fd = recv_fd(stream.as_raw_fd())?;
            // Leak the stream so the connection (and rev's tracking) stays alive
            // for the life of the fd, like a real long-lived compositor session.
            std::mem::forget(stream);
            Ok(fd)
        }
        MessageBody::Error { message } => Err(io::Error::other(message)),
        other => Err(io::Error::other(format!("unexpected reply: {other:?}"))),
    }
}

/// recv_fd, copied from rev/src/seat/fd_passing.rs.
fn recv_fd(socket_fd: RawFd) -> io::Result<RawFd> {
    let mut data_buf = [0u8; 1];
    let mut iov = [io::IoSliceMut::new(&mut data_buf)];
    let mut cmsg_buf = nix::cmsg_space!([RawFd; 8]);
    let m = socket::recvmsg::<UnixAddr>(socket_fd, &mut iov, Some(&mut cmsg_buf), MsgFlags::empty())
        .map_err(|e| io::Error::from_raw_os_error(e as i32))?;
    for c in m.cmsgs()? {
        if let ControlMessageOwned::ScmRights(fds) = c {
            if let Some(fd) = fds.into_iter().next() {
                return Ok(fd);
            }
        }
    }
    Err(io::Error::other("no fd in SCM_RIGHTS"))
}

fn ioctl(fd: RawFd, req: libc::c_ulong, ptr: *mut u8) -> io::Result<()> {
    if unsafe { libc::ioctl(fd, req, ptr) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Full dumb-buffer modeset on the passed fd: pick a connector + mode, allocate
/// a framebuffer, fill it, and SETCRTC. Returns a one-line summary on success.
fn render_frame(fd: RawFd) -> io::Result<String> {
    // 1. resources: a crtc and a connector id.
    let (crtc_id, connector_id) = resources(fd)?;

    // 2. connector's first mode.
    let mode = connector_mode(fd, connector_id)?;
    let w = u16::from_le_bytes(mode[4..6].try_into().unwrap()) as u32; // hdisplay
    let h = u16::from_le_bytes(mode[14..16].try_into().unwrap()) as u32; // vdisplay
    if w == 0 || h == 0 {
        return Err(io::Error::other("connector has no usable mode (disconnected?)"));
    }

    // 3. dumb buffer.
    let mut cd = [0u8; 32];
    cd[0..4].copy_from_slice(&h.to_le_bytes()); // height
    cd[4..8].copy_from_slice(&w.to_le_bytes()); // width
    cd[8..12].copy_from_slice(&32u32.to_le_bytes()); // bpp
    ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, cd.as_mut_ptr())?;
    let handle = u32::from_le_bytes(cd[16..20].try_into().unwrap());
    let pitch = u32::from_le_bytes(cd[20..24].try_into().unwrap());
    let size = u64::from_le_bytes(cd[24..32].try_into().unwrap());

    // 4. framebuffer object.
    let mut fb = [0u8; 28];
    fb[4..8].copy_from_slice(&w.to_le_bytes());
    fb[8..12].copy_from_slice(&h.to_le_bytes());
    fb[12..16].copy_from_slice(&pitch.to_le_bytes());
    fb[16..20].copy_from_slice(&32u32.to_le_bytes()); // bpp
    fb[20..24].copy_from_slice(&24u32.to_le_bytes()); // depth
    fb[24..28].copy_from_slice(&handle.to_le_bytes());
    ioctl(fd, DRM_IOCTL_MODE_ADDFB, fb.as_mut_ptr())?;
    let fb_id = u32::from_le_bytes(fb[0..4].try_into().unwrap());

    // 5. map + fill with a solid colour.
    let mut md = [0u8; 16];
    md[0..4].copy_from_slice(&handle.to_le_bytes());
    ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, md.as_mut_ptr())?;
    let offset = u64::from_le_bytes(md[8..16].try_into().unwrap());
    let map = unsafe {
        libc::mmap(std::ptr::null_mut(), size as usize, libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED, fd, offset as libc::off_t)
    };
    if map == libc::MAP_FAILED {
        return Err(io::Error::other(format!("mmap dumb buffer: {}", io::Error::last_os_error())));
    }
    unsafe {
        let px = map as *mut u32;
        for i in 0..(size as usize / 4) {
            *px.add(i) = 0x00FF_3366; // XRGB8888
        }
    }

    // 6. scan it out.
    let connectors = [connector_id];
    let mut crtc = [0u8; 104];
    crtc[0..8].copy_from_slice(&(connectors.as_ptr() as u64).to_le_bytes()); // set_connectors_ptr
    crtc[8..12].copy_from_slice(&1u32.to_le_bytes()); // count_connectors
    crtc[12..16].copy_from_slice(&crtc_id.to_le_bytes());
    crtc[16..20].copy_from_slice(&fb_id.to_le_bytes());
    crtc[32..36].copy_from_slice(&1u32.to_le_bytes()); // mode_valid
    crtc[36..104].copy_from_slice(&mode);
    ioctl(fd, DRM_IOCTL_MODE_SETCRTC, crtc.as_mut_ptr())?;

    Ok(format!("scanned out a {w}x{h} frame (fb {fb_id}, crtc {crtc_id}, connector {connector_id})"))
}

/// First crtc id and first connector id from MODE_GETRESOURCES.
fn resources(fd: RawFd) -> io::Result<(u32, u32)> {
    let mut res = [0u8; 64];
    ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, res.as_mut_ptr())?;
    let n_crtcs = u32::from_le_bytes(res[36..40].try_into().unwrap());
    let n_conns = u32::from_le_bytes(res[40..44].try_into().unwrap());
    if n_crtcs == 0 || n_conns == 0 {
        return Err(io::Error::other("no crtcs/connectors"));
    }
    let mut crtcs = vec![0u32; n_crtcs as usize];
    let mut conns = vec![0u32; n_conns as usize];
    res = [0u8; 64];
    res[8..16].copy_from_slice(&(crtcs.as_mut_ptr() as u64).to_le_bytes()); // crtc_id_ptr
    res[16..24].copy_from_slice(&(conns.as_mut_ptr() as u64).to_le_bytes()); // connector_id_ptr
    res[36..40].copy_from_slice(&n_crtcs.to_le_bytes());
    res[40..44].copy_from_slice(&n_conns.to_le_bytes());
    ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, res.as_mut_ptr())?;
    Ok((crtcs[0], conns[0]))
}

/// First modeinfo (68 bytes) of a connector via MODE_GETCONNECTOR.
fn connector_mode(fd: RawFd, connector_id: u32) -> io::Result<[u8; 68]> {
    let mut c = [0u8; 80];
    c[48..52].copy_from_slice(&connector_id.to_le_bytes()); // connector_id
    ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, c.as_mut_ptr())?;
    let n_modes = u32::from_le_bytes(c[32..36].try_into().unwrap());
    if n_modes == 0 {
        return Err(io::Error::other("connector reports 0 modes"));
    }
    let mut modes = vec![0u8; n_modes as usize * 68];
    c = [0u8; 80];
    c[8..16].copy_from_slice(&(modes.as_mut_ptr() as u64).to_le_bytes()); // modes_ptr
    c[32..36].copy_from_slice(&n_modes.to_le_bytes()); // count_modes
    c[48..52].copy_from_slice(&connector_id.to_le_bytes());
    ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, c.as_mut_ptr())?;
    let mut first = [0u8; 68];
    first.copy_from_slice(&modes[0..68]);
    Ok(first)
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
