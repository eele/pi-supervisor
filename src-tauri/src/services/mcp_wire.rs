//! hostps protocol layer: minimal HTTP/1.1 + MCP Streamable HTTP (stateless)
//! + the SSE output-stream framing. All JSON/SSE branching lives here so a
//! future switch to a full MCP SDK touches only this module.
//!
//! Negotiation (§3.5.1 of the plan):
//!   Accept: application/json only            → one-shot JSON response
//!   Accept: text/event-stream + progressTok  → SSE stream (progress frames)
//!   Accept: text/event-stream w/o token      → SSE container, single result
//!   non-tools/call methods                   → always JSON

use std::io::{Read, Write};
use std::net::TcpStream;

pub const JSONRPC_VERSION: &str = "2.0";
pub const PROTOCOL_FALLBACK: &str = "2025-03-26";

pub struct HttpReq {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>, // lowercased names
    pub body: Vec<u8>,
}

impl HttpReq {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    pub fn wants_sse(&self) -> bool {
        self.header("accept")
            .map(|a| a.contains("text/event-stream"))
            .unwrap_or(false)
    }
}

/// Read request head + Content-Length body. Caps: 128 KB head, 1 MB body.
pub fn read_request(sock: &mut TcpStream) -> Option<HttpReq> {
    let mut head: Vec<u8> = Vec::new();
    let mut b = [0u8; 1];
    loop {
        match sock.read(&mut b) {
            Ok(0) => return None,
            Ok(_) => {
                head.push(b[0]);
                if head.ends_with(b"\r\n\r\n") {
                    break;
                }
                if head.len() > 128 * 1024 {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
    let text = String::from_utf8_lossy(&head).replace("\r\n", "\n");
    let mut lines = text.split('\n');
    let request_line = lines.next().unwrap_or("");
    let mut pl = request_line.split_whitespace();
    let (Some(method), Some(path)) = (pl.next(), pl.next()) else {
        return None;
    };
    let mut headers = Vec::new();
    let mut content_len = 0usize;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            let k = k.trim().to_lowercase();
            let v = v.trim().to_string();
            if k == "content-length" {
                content_len = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }
    if content_len > 1_000_000 {
        return None;
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
    Some(HttpReq {
        method: method.to_uppercase(),
        path: path.to_string(),
        headers,
        body,
    })
}

pub fn send_json(sock: &mut TcpStream, status: u16, body: &str) {
    let reason = status_reason(status);
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(resp.as_bytes());
    let _ = sock.flush();
}

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    }
}

// ── SSE stream writer ─────────────────────────────────────────────
// chunked transfer-encoding is hand-rolled (std::net has no auto framing).

pub struct SseStream {
    sock: TcpStream,
    dead: bool,
}

impl SseStream {
    /// Emit headers + the immediate `: stream-open` probe comment.
    pub fn start(mut sock: TcpStream) -> Option<SseStream> {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                    Transfer-Encoding: chunked\r\nCache-Control: no-cache\r\n\
                    X-Accel-Buffering: no\r\nConnection: close\r\n\r\n";
        if sock.write_all(head.as_bytes()).is_err() {
            return None;
        }
        let mut s = SseStream { sock, dead: false };
        s.comment("stream-open");
        s.flush();
        Some(s)
    }

    fn chunk(&mut self, data: &[u8]) {
        if self.dead {
            return;
        }
        let frame = format!("{:x}\r\n", data.len());
        if self
            .sock
            .write_all(frame.as_bytes())
            .and_then(|_| self.sock.write_all(data))
            .and_then(|_| self.sock.write_all(b"\r\n"))
            .is_err()
        {
            self.dead = true; // client disconnected mid-stream
        }
    }

    /// SSE comment frame (`: …`) — keepalive / probe, not a protocol message.
    pub fn comment(&mut self, text: &str) {
        let ev = format!(": {text}\n\n");
        self.chunk(ev.as_bytes());
    }

    /// One JSON-RPC message as an `event: message` frame.
    pub fn message(&mut self, json: &serde_json::Value) {
        let ev = format!("event: message\ndata: {json}\n\n");
        self.chunk(ev.as_bytes());
    }

    pub fn flush(&mut self) {
        if !self.dead {
            let _ = self.sock.flush();
        }
    }

    pub fn broken(&self) -> bool {
        self.dead
    }

    pub fn finish(mut self) {
        if !self.dead {
            self.chunk(b"0\r\n\r\n");
            let _ = self.sock.flush();
        }
    }
}

// ── JSON-RPC helpers ──────────────────────────────────────────────

pub fn rpc_result(id: &serde_json::Value, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"jsonrpc": JSONRPC_VERSION, "id": id, "result": result})
}

pub fn rpc_error(id: &serde_json::Value, code: i64, message: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc": JSONRPC_VERSION, "id": id,
        "error": {"code": code, "message": message}})
}

/// MCP tools/call text-content result envelope.
pub fn tool_text_result(text: &str, is_error: bool) -> serde_json::Value {
    serde_json::json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
    })
}

pub fn progress_frame(token: &serde_json::Value, bytes: u64, tail: &str, stdout_chunk: &str, stderr_chunk: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": JSONRPC_VERSION,
        "method": "notifications/progress",
        "params": {
            "progressToken": token,
            "progress": bytes,
            "message": tail,
            "_stream": {"stdout_chunk": stdout_chunk, "stderr_chunk": stderr_chunk},
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_envelopes_match_mcp_shape() {
        let r = tool_text_result("hi", true);
        assert_eq!(r["isError"], true);
        assert_eq!(r["content"][0]["type"], "text");
        let e = rpc_error(&serde_json::json!(7), -32001, "[HOST_PS_GATEWAY_AUTH] invalid token");
        assert_eq!(e["error"]["code"], -32001);
        assert_eq!(e["id"], 7);
    }

    #[test]
    fn progress_frame_carries_stream_and_token() {
        let p = progress_frame(&serde_json::json!("t1"), 12, "tail", "out", "err");
        assert_eq!(p["method"], "notifications/progress");
        assert_eq!(p["params"]["progressToken"], "t1");
        assert_eq!(p["params"]["_stream"]["stdout_chunk"], "out");
    }
}
