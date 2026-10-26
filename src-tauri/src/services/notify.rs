//! D3 — Notify Hub: HTTP :60000, contract-compatible with the legacy
//! node server.js (sandbox-side notify-complete.ts stays untouched).
//!
//!   GET  /              → {"status":"ok","service":"notify"}
//!   POST /notify        → {"title","message"} (accepts text/body for message)
//!   anything else       → 404 {"ok":false,"error":"not found"}
//!
//! Toasts render via PowerShell loading a TOAST XML we wrote to a TEMP
//! FILE — the whole powertoast here-string injection surface (backtick /
//! $ expansion corrupting XML) is designed out: user text never reaches a
//! command line, only the fixed file path we control.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use crate::util::{run_cmd, Flag};

pub const PORT: u16 = 60000;
const MAX_BODY: usize = 1_000_000; // same 1 MB cap as server.js

pub fn bind() -> Result<TcpListener, String> {
    TcpListener::bind(("0.0.0.0", PORT))
        .map_err(|e| format!("cannot bind port {PORT} ({e}) — notify already running?"))
}

pub fn run(listener: TcpListener, stop: Arc<Flag>) {
    let _ = listener.set_nonblocking(true);
    loop {
        // Checked at the TOP: an idle WouldBlock→sleep cycle would never
        // reach the error arm below, and stop_listener's join() would hang.
        if !stop.get() {
            return;
        }
        match listener.accept() {
            Ok((sock, _)) => {
                let spawn = std::thread::Builder::new()
                    .stack_size(2 * 1024 * 1024)
                    .spawn(move || {
                        let _ = std::panic::catch_unwind(move || {
                            handle_conn(sock);
                        });
                    });
                let _ = spawn;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(80));
            }
            Err(_) if stop.get() => return,
            Err(_) => {}
        }
    }
}

fn handle_conn(mut sock: TcpStream) {
    let _ = sock.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let _ = sock.set_write_timeout(Some(std::time::Duration::from_secs(10)));

    // Read request head (method/path/headers) — cap at 64 KB.
    let mut head: Vec<u8> = Vec::new();
    let mut b = [0u8; 1];
    loop {
        match sock.read(&mut b) {
            Ok(0) => return,
            Ok(_) => {
                head.push(b[0]);
                if head.ends_with(b"\r\n\r\n") {
                    break;
                }
                if head.len() > 64 * 1024 {
                    return;
                }
            }
            Err(_) => return,
        }
    }
    let head_s = String::from_utf8_lossy(&head).to_lowercase();
    let first_line = head_s.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return;
    };
    let content_len = head_s
        .lines()
        .find_map(|l| {
            l.strip_prefix("content-length:")
                .and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    if content_len > MAX_BODY {
        send_json(&mut sock, 400, r#"{"ok":false,"error":"body too large"}"#);
        return;
    }

    let mut body = vec![0u8; content_len];
    let mut off = 0;
    while off < content_len {
        match sock.read(&mut body[off..]) {
            Ok(0) => break,
            Ok(n) => off += n,
            Err(_) => break,
        }
    }

    match (method, path) {
        ("get", "/") => send_json(
            &mut sock,
            200,
            r#"{"status":"ok","service":"notify"}"#,
        ),
        ("post", "/notify") => handle_notify(&mut sock, &body),
        _ => send_json(&mut sock, 404, r#"{"ok":false,"error":"not found"}"#),
    }
}

fn handle_notify(sock: &mut TcpStream, body: &[u8]) {
    if !body.is_empty() && serde_json::from_slice::<serde_json::Value>(body).is_err() {
        return send_json(sock, 400, r#"{"ok":false,"error":"invalid JSON body"}"#);
    }
    let parsed: serde_json::Value = if body.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(body).unwrap_or(serde_json::json!({}))
    };
    let field = |v: &serde_json::Value| -> String {
        match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            _ => String::new(),
        }
    };
    let title = field(parsed.get("title").unwrap_or(&serde_json::Value::Null)).trim().to_string();
    let message = ["message", "text", "body"]
        .iter()
        .map(|k| parsed.get(*k).unwrap_or(&serde_json::Value::Null))
        .find(|v| !v.is_null() && !field(v).trim().is_empty())
        .map(field)
        .unwrap_or_default()
        .trim()
        .to_string();

    if title.is_empty() || message.is_empty() {
        // Exact legacy error string (contract parity with server.js).
        return send_json(
            sock,
            400,
            r#"{"ok":false,"error":"缺少参数：需要 title(标题) 和 message/text(文本)"}"#,
        );
    }

    let xml = build_toast_xml(&title, &message);
    match show_toast(&xml) {
        Ok(()) => {
            let out = serde_json::json!({"ok": true, "title": title, "message": message});
            send_json(sock, 200, &out.to_string());
        }
        Err(e) => {
            let out = serde_json::json!({"ok": false, "error": format!("推送失败: {e}")});
            send_json(sock, 500, &out.to_string());
        }
    }
}

/// XML-escape text for a standard XML text node.
pub fn xml_escape(s: &str) -> String {
    // Sanitize like server.js did: drop XML-illegal control chars, fold new
    // lines to spaces (toasts don't render them). Lone surrogates cannot
    // exist in Rust strings by construction.
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\r' || *c == '\t')
        .collect();
    let cleaned = cleaned
        .replace("\r\n", " ")
        .replace('\r', " ")
        .replace('\n', " ")
        .replace('\t', " ");
    cleaned
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Toast payload mirroring the powertoast config: reminder scenario (sticky),
/// attribution line, OK→dismiss system action, default sound.
pub fn build_toast_xml(title: &str, message: &str) -> String {
    format!(
        "<toast scenario=\"reminder\" launch=\"dismiss\" activationType=\"system\" duration=\"short\">\
         <visual><binding template=\"ToastGeneric\">\
         <text>{}</text><text>{}</text>\
         <text placement=\"attribution\">notify-service</text>\
         </binding></visual>\
         <audio src=\"ms-winsoundevent:Notification.Default\" />\
         <actions><action content=\"OK\" activationType=\"system\" arguments=\"dismiss\" /></actions>\
         </toast>",
        xml_escape(title),
        xml_escape(message)
    )
}

/// PowerShell WinRT toast: loads OUR temp XML file (fixed literal path —
/// no user text on the command line, hence nothing to inject).
fn show_toast(xml: &str) -> Result<(), String> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pi-sup-toast-{n}.xml"));
    // BOM matters: PowerShell 5.1 Get-Content reads a BOM-less file as ANSI
    // (GBK here), mangling UTF-8 CJK into XML-illegal chars — LoadXml then
    // fails 0xC00CE56D and every Chinese notify 500s. With the BOM,
    // Get-Content decodes UTF-8 and strips it before LoadXml sees the text.
    let mut doc = vec![0xEFu8, 0xBB, 0xBF];
    doc.extend_from_slice(xml.as_bytes());
    std::fs::write(&path, doc).map_err(|e| e.to_string())?;

    let ps = format!(
        "$ErrorActionPreference='Stop';\
         [void][Windows.UI.Notifications.ToastNotificationManager,Windows.UI.Notifications,ContentType=WindowsRuntime];\
         [void][Windows.Data.Xml.Dom.XmlDocument,Windows.Data.Xml.Dom,ContentType=WindowsRuntime];\
         $x = New-Object Windows.Data.Xml.Dom.XmlDocument;\
         $x.LoadXml((Get-Content -LiteralPath '{}' -Raw));\
         $t = New-Object Windows.UI.Notifications.ToastNotification $x;\
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Microsoft.Windows.Explorer').Show($t)",
        path.display().to_string().replace('\'', "''")
    );
    let r = run_cmd(
        "powershell",
        &["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), ps],
        None,
        20_000,
    );
    let _ = std::fs::remove_file(&path);
    r.map(|_| ())
}

fn send_json(sock: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(resp.as_bytes());
    let _ = sock.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_beats_the_powertoast_attack_samples() {
        // Samples from server.js comments: ]]> close, here-string killers,
        // control chars, xml tags. None may survive raw.
        let evil = "a\u{0}b\u{1}\u{c}]]>@c`0`a$pid$(calc)<xml>&amp;<>";
        let out = xml_escape(evil);
        assert!(!out.contains("]]>"));
        assert!(!out.contains('<'));
        assert!(!out.contains(">"));
        assert!(!out.chars().any(|c| c.is_control()));
        assert!(out.contains("&lt;xml&gt;"));
        // Raw text round-trips escaped:
        assert!(out.contains("&amp;amp;"));
    }

    #[test]
    fn newlines_fold_to_space() {
        assert_eq!(xml_escape("a\r\nb\tc"), "a b c");
    }

    #[test]
    fn toast_xml_is_well_formed() {
        let xml = build_toast_xml("T<i>", "m & ]]> x");
        assert!(xml.starts_with("<toast"));
        assert!(xml.ends_with("</toast>"));
        assert!(!xml.contains("<i>"));
    }
}
