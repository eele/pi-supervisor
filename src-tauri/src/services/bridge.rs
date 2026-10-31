//! D2 — Agent TCP Bridge: native Rust port of pageant-tcp-bridge.py.
//!
//! Exposes the host Pageant over TCP 60022 for sandboxes that cannot dial
//! Windows named pipes. Wire protocol (contract with nook-agent-relay.py,
//! byte-for-byte compatible with the python version):
//!   client → 32-byte token line + "\n", then OpenSSH agent frames
//!   (uint32 BE length || payload), strictly one request per response.
//! Token lives in ~/.ssh/agent-bridge-token and is re-read per connection.
//! Frames pass through untranslated: the sandbox's Ubuntu OpenSSH and
//! Pageant both speak PuTTY's agent numbering (identities 11/12, sign
//! 13/14) — verified by wire capture from inside the sandbox.
//!
//! Pageant channel: the documented WM_COPYDATA agent protocol — a named
//! file mapping (PageantRequest%08x) carries the framing; the mapping NAME
//! travels in COPYDATASTRUCT (dwData == 0x804e50ba).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::util::Flag;

pub const DEFAULT_PORT: u16 = 60022;
const AGENT_MAX_MSGLEN: usize = 262_144;

fn token_path() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(|p| std::path::PathBuf::from(p).join(".ssh").join("agent-bridge-token"))
}

/// Bind the listener on the IPv6 any-address (dual-stack on Windows when
/// IPV6_V6ONLY is off — the default for `::` bind via std is v6only=ON on
/// Windows, so we additionally listen on v4: two listeners, one loop).
/// Plain bind (no SO_REUSEADDR): a second instance fails HERE — that IS the
/// single-instance guarantee the python bridge relied on.
pub fn bind(port: u16) -> Result<Vec<TcpListener>, String> {
    let v6 = TcpListener::bind(("::", port));
    let v4 = TcpListener::bind(("0.0.0.0", port));
    match (v6, v4) {
        (Ok(a), Ok(b)) => Ok(vec![a, b]),
        (Ok(a), Err(_)) => Ok(vec![a]),
        (Err(_), Ok(b)) => Ok(vec![b]),
        (Err(e6), Err(e4)) => Err(format!(
            "cannot bind port {port} ({e6} / {e4}) — bridge already running?"
        )),
    }
}

/// Accept loop over one or more listeners; returns when `stop` flips.
/// Each connection runs in its own thread (same shape as the python
/// version's per-connection Thread).
pub fn run(listeners: Vec<TcpListener>, stop: Arc<Flag>) {
    for l in &listeners {
        let _ = l.set_nonblocking(true);
    }
    loop {
        let mut busy = false;
        for l in &listeners {
            match l.accept() {
                Ok((sock, _)) => {
                    busy = true;
                    // Windows ACCEPTED sockets inherit the listener's
                    // non-blocking mode. Flip back to blocking: otherwise
                    // relay()'s next read hits WouldBlock the instant the
                    // client pauses, takes the Err(_) exit, and drops the
                    // socket with the just-written response still queued
                    // (clients see WSAECONNABORTED instead of the reply).
                    let _ = sock.set_nonblocking(false);
                    let tp = token_path();
                    let spawn = std::thread::Builder::new()
                        .stack_size(2 * 1024 * 1024)
                        .spawn(move || {
                            let _ = std::panic::catch_unwind(move || {
                                handle_conn(sock, tp.as_deref());
                            });
                        });
                    let _ = spawn; // thread bomb-out just drops the client
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) if !stop.get() => return,
                Err(_) => {}
            }
        }
        // `stop` is an ALIVE flag (spawned true, flipped false by
        // stop_listener) — the same convention notify.rs uses. Without the
        // negation the thread returned on its very first pass and the
        // watchdog resurrect loop made bridge never listen.
        if !stop.get() {
            return;
        }
        if !busy {
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    }
}

fn handle_conn(mut sock: TcpStream, token_file: Option<&std::path::Path>) {
    // Token is re-read per connection: rotating the file takes effect live.
    let want = match token_file.map(|p| std::fs::read(p)).and_then(|r| r.ok()) {
        Some(raw) => raw.trim_ascii_vec(),
        None => return,
    };
    let got = match read_until_newline(&mut sock) {
        Some(t) => t.trim_ascii_vec(),
        None => return,
    };
    if want.is_empty() || got != want {
        return; // bad/absent token: silent drop, like the python bridge
    }
    relay(&mut sock);
}

fn read_until_newline(s: &mut TcpStream) -> Option<Vec<u8>> {
    let mut tok = Vec::new();
    let mut b = [0u8; 1];
    while tok.len() < 64 {
        match s.read(&mut b) {
            Ok(0) => return None,
            Ok(_) => {
                tok.push(b[0]);
                if b[0] == b'\n' {
                    return Some(tok);
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    None
}

fn relay(sock: &mut TcpStream) {
    let mut len_buf = [0u8; 4];
    loop {
        if !read_exact_or_close(sock, &mut len_buf) {
            return;
        }
        let ln = u32::from_be_bytes(len_buf) as usize;
        if ln == 0 || ln > AGENT_MAX_MSGLEN - 4 {
            return;
        }
        let mut frame = vec![0u8; 4 + ln];
        frame[..4].copy_from_slice(&len_buf);
        if !read_exact_or_close(sock, &mut frame[4..]) {
            return;
        }
        // No message-number translation: the sandbox's ssh/ssh-add are
        // Ubuntu-patched OpenSSH that already speak PuTTY's numbering
        // (wire capture: identities request = 11, answer = 12) — the
        // same dialect Pageant speaks natively. Passthrough is exact.
        let resp = match pageant_request(&frame) {
            Ok(r) => r,
            Err(_) => vec![0, 0, 0, 1, 5], // SSH_AGENT_FAILURE
        };
        if sock.write_all(&resp).is_err() {
            return;
        }
    }
}

fn read_exact_or_close(s: &mut TcpStream, buf: &mut [u8]) -> bool {
    let mut off = 0;
    while off < buf.len() {
        match s.read(&mut buf[off..]) {
            Ok(0) => return false,
            Ok(n) => off += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
    true
}

static MAP_SEQ: AtomicU32 = AtomicU32::new(0);

/// WM_COPYDATA round trip with Pageant. Input/output are uint32-framed.
#[cfg(target_os = "windows")]
pub fn pageant_request(framing: &[u8]) -> Result<Vec<u8>, String> {
    use std::ffi::c_void;
    use windows::core::w;
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, LPARAM, WPARAM};
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::System::Memory::{
        CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS, PAGE_READWRITE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SendMessageW, WM_COPYDATA};

    const AGENT_COPYDATA_ID: usize = 0x804e_50ba;

    if framing.len() > AGENT_MAX_MSGLEN - 4 {
        return Err("request too large".into());
    }

    // Find every time: survives a Pageant restart across a live connection.
    let hwnd = unsafe { FindWindowW(w!("Pageant"), w!("Pageant")) }
        .ok()
        .filter(|h| !h.is_invalid());
    let Some(hwnd) = hwnd else {
        return Err("Pageant not running".into());
    };

    let mapname = format!("PageantRequest{:08x}", MAP_SEQ.fetch_add(1, Ordering::Relaxed));
    let map_wide: Vec<u16> = mapname.encode_utf16().chain(std::iter::once(0)).collect();
    // The mapping name handed to Pageant itself is ASCII + NUL.
    let name_ascii: Vec<u8> = mapname.bytes().chain(std::iter::once(0)).collect();

    let hmap = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            None,
            PAGE_READWRITE,
            0,
            AGENT_MAX_MSGLEN as u32,
            windows::core::PCWSTR(map_wide.as_ptr()),
        )
    }
    .map_err(|e| format!("CreateFileMapping failed: {e}"))?;

    let view = unsafe { MapViewOfFile(hmap, FILE_MAP_ALL_ACCESS, 0, 0, AGENT_MAX_MSGLEN) };
    if view.Value.is_null() {
        unsafe {
            let _ = CloseHandle(hmap);
        }
        return Err("MapViewOfFile failed".into());
    }

    let result = unsafe {
        std::ptr::copy_nonoverlapping(framing.as_ptr(), view.Value as *mut u8, framing.len());
        let cds = COPYDATASTRUCT {
            dwData: AGENT_COPYDATA_ID,
            cbData: name_ascii.len() as u32,
            lpData: name_ascii.as_ptr() as *mut c_void,
        };
        let res = SendMessageW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const _ as isize),
        );
        if res.0 <= 0 {
            Err(format!("pageant rejected (result {})", res.0))
        } else {
            let base = view.Value as *const u8;
            let mut lb = [0u8; 4];
            std::ptr::copy_nonoverlapping(base, lb.as_mut_ptr(), 4);
            let rlen = u32::from_be_bytes(lb) as usize;
            if rlen == 0 || rlen > AGENT_MAX_MSGLEN - 4 {
                Err(format!("bad response len {rlen}"))
            } else {
                let mut out = vec![0u8; 4 + rlen];
                std::ptr::copy_nonoverlapping(base, out.as_mut_ptr(), 4 + rlen);
                Ok(out)
            }
        }
    };

    unsafe {
        let _ = UnmapViewOfFile(view);
        let _ = CloseHandle(hmap);
    }
    result
}

#[cfg(not(target_os = "windows"))]
pub fn pageant_request(_framing: &[u8]) -> Result<Vec<u8>, String> {
    Err("pageant bridge is Windows-only".into())
}

/// std-only ascii trim for byte vectors (slice::trim_ascii is unstable here).
pub trait TrimAscii {
    fn trim_ascii_vec(&self) -> Vec<u8>;
}
impl TrimAscii for [u8] {
    fn trim_ascii_vec(&self) -> Vec<u8> {
        let start = self
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(self.len());
        let end = self
            .iter()
            .rposition(|b| !b.is_ascii_whitespace())
            .map(|i| i + 1)
            .unwrap_or(start);
        self[start..end].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_compare_tolerates_crlf() {
        let got = b"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\n";
        let want = b"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\r\n";
        assert_eq!(got.trim_ascii_vec(), want.trim_ascii_vec());
    }

    #[test]
    fn bad_frame_len_rejected() {
        // Contract: 0 is invalid, and > MAX-4 is invalid; MAX-4 is the
        // largest legal payload length (python parity).
        let rejected = |ln: u32| ln as usize == 0 || ln as usize > AGENT_MAX_MSGLEN - 4;
        assert!(rejected(0));
        assert!(rejected(262_141));
        assert!(rejected(262_144));
        assert!(!rejected(262_140));
        assert!(!rejected(1));
    }
}
