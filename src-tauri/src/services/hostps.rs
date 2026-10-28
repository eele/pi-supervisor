//! hostps — Host PowerShell MCP gateway (plan pi-host-powershell-mcp-plan).
//!
//! MCP Streamable HTTP, stateless, dual-mode on a SINGLE always-on listener:
//!   * running  → full gateway: auth → tools/call → powershell subprocess
//!   * stopped  → access-disabled responder: authenticated callers get a
//!     normal MCP result [HOST_PS_GATEWAY_DISABLED] ("no permission", agent
//!     keeps working), never a connection error. The port is only released
//!     when the supervisor process itself dies — that is what lets the
//!     sandbox tell "user turned it off" apart from "gateway is down".
//!
//! Auth: `Authorization: Bearer <HOST_PS_KEY>` verified against
//! HKCU\Environment on EVERY request (live rotation, no restart). Wrong
//! token → 401 + auth_failed audit row (fingerprint only, never the token).
//!
//! Audit discipline: tools/call INSERTs `started` BEFORE executing — an
//! unwritable audit DB refuses execution outright (LOG_FAILED). Every other
//! JSON-RPC request lands exactly one terminal row.

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::psaudit::PsAudit;
use crate::services::mcp_wire as wire;
use crate::util::{self, Flag};

// Default port lives in config.rs (services.hostps_port); the listener is
// always bound to that configured value.
const MCP_PATH: &str = "/mcp";
const MCP_OUTPUT_CAP: usize = 64 * 1024; // per-stream cap in the MCP reply

pub const AUTH_PREFIX: &str = "[HOST_PS_GATEWAY_AUTH]";
pub const EXEC_PREFIX: &str = "[HOST_PS_GATEWAY_EXEC_FAILED]";
pub const TIMEOUT_PREFIX: &str = "[HOST_PS_GATEWAY_TIMEOUT]";
pub const LOG_PREFIX: &str = "[HOST_PS_GATEWAY_LOG_FAILED]";
pub const DISABLED_PREFIX: &str = "[HOST_PS_GATEWAY_DISABLED]";

/// Class-A (connection/fault) action clause appended to error texts.
pub const PAUSE_ACTION: &str = " — ACTION FOR AGENT: pause this task, do NOT retry more than once, do NOT fabricate results or substitute sandbox-local commands; notify the user that the host-powershell MCP is unavailable and include this error.";
/// Class-B (user intent) text — deliberately the OPPOSITE instruction.
pub const DISABLED_TEXT: &str = "[HOST_PS_GATEWAY_DISABLED] Host PowerShell access is turned OFF by the user in Pi Supervisor — this is an intentional user setting, NOT a connection failure. ACTION FOR AGENT: you currently have NO permission to run host PowerShell; do NOT pause reasoning, do NOT report an incident; continue with sandbox-local tools and simply mention that host PowerShell is switched off.";

pub struct Ctx {
    pub enabled: AtomicBool,
    /// "auto" | "force" | "off" (config services.hostps_sse).
    pub sse_mode: String,
    pub started: Instant,
    pub audit: Arc<PsAudit>,
    /// powershell pids currently executing (killed on Stop).
    pub in_flight: Mutex<Vec<u32>>,
    pub port: u16,
}

impl Ctx {
    pub fn new(audit: Arc<PsAudit>, sse_mode: String, port: u16, enabled: bool) -> Ctx {
        Ctx {
            enabled: AtomicBool::new(enabled),
            sse_mode,
            started: Instant::now(),
            audit,
            in_flight: Mutex::new(Vec::new()),
            port,
        }
    }

    pub fn kill_in_flight(&self) {
        let pids: Vec<u32> = self.in_flight.lock().map(|v| v.clone()).unwrap_or_default();
        for pid in pids {
            let _ = util::run_cmd(
                "taskkill",
                &["/PID".into(), pid.to_string(), "/T".into(), "/F".into()],
                None,
                5_000,
            );
        }
    }
}

/// Dual-stack bind like bridge.rs (Windows `::` is v6-only, so v4 too).
pub fn bind(port: u16) -> Result<Vec<TcpListener>, String> {
    let v6 = TcpListener::bind(("::", port));
    let v4 = TcpListener::bind(("0.0.0.0", port));
    match (v6, v4) {
        (Ok(a), Ok(b)) => Ok(vec![a, b]),
        (Ok(a), Err(_)) => Ok(vec![a]),
        (Err(_), Ok(b)) => Ok(vec![b]),
        (Err(e6), Err(e4)) => Err(format!("cannot bind port {port} ({e6} / {e4}) — hostps already listening?")),
    }
}

/// Accept loop; lives for the whole supervisor process (the disabled
/// responder must never go quiet — see plan §3.4).
pub fn run(listeners: Vec<TcpListener>, ctx: Arc<Ctx>, stop: Arc<Flag>) {
    for l in &listeners {
        let _ = l.set_nonblocking(true);
    }
    let mut last_purge = Instant::now();
    loop {
        let mut busy = false;
        for l in &listeners {
            match l.accept() {
                Ok((sock, peer)) => {
                    busy = true;
                    let ctx = ctx.clone();
                    let spawn = std::thread::Builder::new()
                        .stack_size(2 * 1024 * 1024)
                        .spawn(move || {
                            let _ = std::panic::catch_unwind(move || {
                                handle_conn(sock, peer.to_string(), ctx);
                            });
                        });
                    let _ = spawn;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) if !stop.get() => return,
                Err(_) => {}
            }
        }
        // alive-flag convention (see bridge.rs): false = stop requested.
        if !stop.get() {
            return;
        }
        // Retention pass roughly daily.
        if last_purge.elapsed() > Duration::from_secs(86_400) {
            last_purge = Instant::now();
            let _ = ctx.audit.purge();
        }
        if !busy {
            std::thread::sleep(Duration::from_millis(80));
        }
    }
}

fn handle_conn(mut sock: TcpStream, peer: String, ctx: Arc<Ctx>) {
    let _ = sock.set_read_timeout(Some(Duration::from_secs(660)));
    let _ = sock.set_write_timeout(Some(Duration::from_secs(30)));
    let Some(req) = wire::read_request(&mut sock) else { return };
    if req.method != "POST" || req.path != MCP_PATH {
        return wire::send_json(&mut sock, 404, r#"{"ok":false,"error":"not found"}"#);
    }
    let parsed: Value = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(_) => {
            return wire::send_json(
                &mut sock,
                400,
                &wire::rpc_error(&Value::Null, -32700, "parse error").to_string(),
            )
        }
    };
    let id = parsed.get("id").cloned().unwrap_or(Value::Null);
    let method = parsed.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let ua = req.header("user-agent").unwrap_or("").to_string();
    let client = format!("{peer} {ua}");

    // ── auth first (both modes): wrong token is 401 + auth_failed row ──
    let token_ok = check_auth(req.header("authorization"));
    if !token_ok {
        let fp = bearer_fp(req.header("authorization"));
        let _ = ctx.audit.log_simple(
            &new_request_id(), &client, "auth", None, None, "auth_failed",
            Some(&format!("{AUTH_PREFIX} invalid token fp {fp}")),
        );
        return wire::send_json(
            &mut sock,
            401,
            &wire::rpc_error(&id, -32001, &format!("{AUTH_PREFIX} invalid token")).to_string(),
        );
    }

    // notifications never get a JSON-RPC reply (HTTP 202)
    if method == "notifications/initialized" {
        let _ = ctx.audit.log_simple(&new_request_id(), &client, &method, None, None, "ok", None);
        return wire::send_json(&mut sock, 202, "");
    }

    let args_json = parsed.get("params").map(|p| p.to_string());
    let tool = if method == "tools/call" {
        parsed
            .pointer("/params/name")
            .and_then(|n| n.as_str())
            .map(String::from)
    } else {
        None
    };

    if !ctx.enabled.load(Ordering::Relaxed) {
        return handle_disabled(&mut sock, &id, &method, tool.as_deref(), args_json.as_deref(), &client, &ctx);
    }

    match method.as_str() {
        "initialize" => {
            let requested = parsed
                .pointer("/params/protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or(wire::PROTOCOL_FALLBACK);
            let version = if requested >= "2024-11-05" { requested } else { wire::PROTOCOL_FALLBACK };
            let result = serde_json::json!({
                "protocolVersion": version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "host-ps-gateway", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Host PowerShell runs on the real Windows machine. Treat [HOST_PS_GATEWAY_*] errors per the AGENTS.md rules.",
            });
            let _ = ctx.audit.log_simple(&new_request_id(), &client, &method, None, args_json.as_deref(), "ok", None);
            wire::send_json(&mut sock, 200, &wire::rpc_result(&id, result).to_string());
        }
        "ping" => {
            let _ = ctx.audit.log_simple(&new_request_id(), &client, &method, None, None, "ok", None);
            wire::send_json(&mut sock, 200, &wire::rpc_result(&id, serde_json::json!({})).to_string());
        }
        "tools/list" => {
            let _ = ctx.audit.log_simple(&new_request_id(), &client, &method, None, None, "ok", None);
            wire::send_json(&mut sock, 200, &wire::rpc_result(&id, serde_json::json!({"tools": tools_spec()})).to_string());
        }
        "tools/call" => handle_tools_call(sock, &parsed, &id, &client, args_json.as_deref(), req.wants_sse(), &ctx),
        other => {
            let _ = ctx.audit.log_simple(&new_request_id(), &client, other, None, args_json.as_deref(), "tool_error", Some("unknown method"));
            wire::send_json(&mut sock, 200, &wire::rpc_error(&id, -32601, &format!("method not found: {other}")).to_string());
        }
    }
}

/// Disabled-mode responses (authenticated). tools/list + initialize stay
/// functional so pi's connection stays healthy; powershell/health get the
/// class-B DISABLED answer. All others normal.
fn handle_disabled(
    sock: &mut TcpStream,
    id: &Value,
    method: &str,
    tool: Option<&str>,
    args_json: Option<&str>,
    client: &str,
    ctx: &Ctx,
) {
    match method {
        "initialize" | "tools/list" | "ping" => {
            // Same shape as running mode (clients must not see a difference).
            if method == "initialize" {
                let result = serde_json::json!({
                    "protocolVersion": wire::PROTOCOL_FALLBACK,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "host-ps-gateway", "version": env!("CARGO_PKG_VERSION")},
                });
                let _ = ctx.audit.log_simple(&new_request_id(), client, method, None, args_json, "ok", None);
                wire::send_json(sock, 200, &wire::rpc_result(id, result).to_string());
            } else if method == "tools/list" {
                let _ = ctx.audit.log_simple(&new_request_id(), client, method, None, None, "ok", None);
                wire::send_json(sock, 200, &wire::rpc_result(id, serde_json::json!({"tools": tools_spec()})).to_string());
            } else {
                let _ = ctx.audit.log_simple(&new_request_id(), client, method, None, None, "ok", None);
                wire::send_json(sock, 200, &wire::rpc_result(id, serde_json::json!({})).to_string());
            }
        }
        "tools/call" if tool == Some("health") => {
            let _ = ctx.audit.log_simple(&new_request_id(), client, method, tool, args_json, "disabled", None);
            wire::send_json(
                sock,
                200,
                &wire::rpc_result(id, health_result(ctx, true)).to_string(),
            );
        }
        "tools/call" => {
            let _ = ctx.audit.log_simple(&new_request_id(), client, method, tool, args_json, "disabled", Some(DISABLED_PREFIX));
            wire::send_json(
                sock,
                200,
                &wire::rpc_result(id, wire::tool_text_result(DISABLED_TEXT, true)).to_string(),
            );
        }
        other => {
            let _ = ctx.audit.log_simple(&new_request_id(), client, other, None, args_json, "disabled", None);
            wire::send_json(sock, 200, &wire::rpc_result(id, serde_json::json!({})).to_string());
        }
    }
}

fn handle_tools_call(
    mut sock: TcpStream,
    parsed: &Value,
    id: &Value,
    client: &str,
    args_json: Option<&str>,
    wants_sse: bool,
    ctx: &Arc<Ctx>,
) {
    let Some(tool) = parsed.pointer("/params/name").and_then(|n| n.as_str()) else {
        return wire::send_json(&mut sock, 200, &wire::rpc_error(id, -32602, "missing tool name").to_string());
    };
    let request_id = new_request_id();

    if tool == "health" {
        let _ = ctx.audit.log_simple(&request_id, client, "tools-call", Some(tool), args_json, "ok", None);
        return wire::send_json(&mut sock, 200, &wire::rpc_result(id, health_result(ctx, false)).to_string());
    }
    if tool != "powershell" {
        let _ = ctx.audit.log_simple(&request_id, client, "tools-call", Some(tool), args_json, "tool_error", Some("unknown tool"));
        return wire::send_json(
            &mut sock,
            200,
            &wire::rpc_result(id, wire::tool_text_result(&format!("{EXEC_PREFIX} unknown tool: {tool}"), true)).to_string(),
        );
    }

    let script = parsed
        .pointer("/params/arguments/script")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let timeout_s = parsed
        .pointer("/params/arguments/timeout_seconds")
        .and_then(|t| t.as_u64())
        .unwrap_or(120)
        .min(600);

    // SSE negotiation (§3.5.1): Accept carries the stream capability, the
    // progressToken gates progress frames (mode may override both).
    let progress_token = parsed.pointer("/params/_meta/progressToken");
    let use_sse = sse_for(ctx, parsed, wants_sse);
    let stream_flag = use_sse && progress_token.is_some();

    // LOG_FAILED guard: 抢占式 started 行先落库，写不进 = 拒绝执行。
    let row = match ctx.audit.log_start(&request_id, client, tool, args_json.unwrap_or("{}"), stream_flag) {
        Ok(r) => r,
        Err(e) => {
            return wire::send_json(
                &mut sock,
                200,
                &wire::rpc_result(
                    id,
                    wire::tool_text_result(&format!("{LOG_PREFIX} audit unavailable: {e}{PAUSE_ACTION}"), true),
                )
                .to_string(),
            );
        }
    };

    let started = Instant::now();
    match use_sse {
        true => stream_exec(sock, id, &script, timeout_s, started, row, progress_token, ctx),
        false => batch_exec(&mut sock, id, &script, timeout_s, started, row, ctx),
    }
}

/// §3.5.1: SSE container when the client asked for it (Accept) or supplied a
/// progressToken; off disables, force enables regardless. Without a token
/// the stream only carries the final result frame (equivalent to batch).
fn sse_for(ctx: &Ctx, parsed: &Value, accept_sse: bool) -> bool {
    match ctx.sse_mode.as_str() {
        "off" => false,
        "force" => true,
        _ => accept_sse || parsed.pointer("/params/_meta/progressToken").is_some(),
    }
}

struct ExecResult {
    stdout: String,
    stderr: String,
    exit: Option<i64>,
    timed_out: bool,
    disconnected: bool,
}

/// Shared subprocess runner. Reader threads push chunks; the caller drains
/// with a deadline and optional per-chunk callback (the SSE sink).
fn run_ps<F: FnMut(&str, &[u8]) -> bool>(
    script: &str,
    timeout_s: u64,
    ctx: &Ctx,
    mut on_chunk: F,
) -> Result<ExecResult, String> {
    // $ProgressPreference: with stderr piped, PowerShell 5.1 serializes
    // progress records as CLIXML into stderr (visible noise in every
    // tools/call result). SilentlyContinue kills the source at the root.
    const PRELUDE: &str =
        "$ProgressPreference='SilentlyContinue'; [Console]::OutputEncoding=[Text.Encoding]::UTF8; ";
    let full = format!("{PRELUDE}{script}");
    let encoded = b64_utf16le(&full);
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot spawn powershell: {e}"))?;
    let pid = child.id();
    ctx.in_flight.lock().map(|mut v| v.push(pid)).ok();

    let (tx, rx) = mpsc::channel::<(u8, Vec<u8>)>();
    if let Some(mut out) = child.stdout.take() {
        let tx = tx.clone();
        std::thread::spawn(move || pump_pipe(&mut out, 0, tx));
    }
    if let Some(mut err) = child.stderr.take() {
        let tx = tx.clone();
        std::thread::spawn(move || pump_pipe(&mut err, 1, tx));
    }
    drop(tx);

    let deadline = Instant::now() + Duration::from_secs(timeout_s);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut timed_out = false;
    let mut disconnected = false;
    let mut exit: Option<i64> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok((which, chunk)) => {
                let alive = if which == 0 {
                    let a = on_chunk("o", &chunk);
                    stdout.extend_from_slice(&chunk);
                    a
                } else {
                    let a = on_chunk("e", &chunk);
                    stderr.extend_from_slice(&chunk);
                    a
                };
                if !alive {
                    // SSE sink died (client vanished): kill tree, no orphans.
                    disconnected = true;
                    kill_tree(pid);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline && !timed_out {
                    timed_out = true;
                    kill_tree(pid);
                }
            }
        }
        if exit.is_none() {
            if let Ok(Some(st)) = child.try_wait() {
                exit = code_of(&st);
            }
        }
    }
    // Readers are gone (EOF); reap.
    let exit = match exit {
        Some(c) => Some(c),
        None => child.wait().ok().and_then(|s| code_of(&s)),
    };
    ctx.in_flight.lock().map(|mut v| v.retain(|p| *p != pid)).ok();
    Ok(ExecResult {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: strip_clixml(&String::from_utf8_lossy(&stderr)),
        exit,
        timed_out,
        disconnected,
    })
}

/// Second-line defense against PS 5.1 CLIXML stderr serialization (the
/// PRELUDE already silences progress records): drop the `#< CLIXML`
/// marker and every `<Objs …>…</Objs>` record blob, but RECOVER the real
/// error records serialized inside it (`<S S="Error">…</S>` elements,
/// one per line, `_x000D_/_x000A_` escapes) — plain stderr text that
/// follows the blob is kept as-is.
fn strip_clixml(s: &str) -> String {
    if !s.contains("CLIXML") && !s.contains("<Objs") {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<Objs") {
        out.push_str(&rest[..i]);
        let (blob, tail) = match rest[i..].find("</Objs>") {
            Some(j) => (&rest[i..i + j], &rest[i + j + "</Objs>".len()..]),
            None => (&rest[i..], ""),
        };
        // Error records: <S S="Error">line</S> (attribute order as PS
        // emits it; value may contain escaped XML entities).
        let mut r = blob;
        while let Some(a) = r.find("<S S=\"Error\">") {
            r = &r[a + "<S S=\"Error\">".len()..];
            match r.find("</S>") {
                Some(b) => {
                    let line = decode_clixml_escapes(&r[..b]);
                    out.push_str(&line);
                    out.push('\n');
                    r = &r[b + 4..];
                }
                None => break,
            }
        }
        rest = tail;
    }
    out.push_str(rest);
    let t = out.replace("#< CLIXML", "");
    // Drop the CLIXML preamble line PS prints before the first blob.
    t.lines()
        .filter(|l| !l.trim().starts_with("#< CLIXML"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn decode_clixml_escapes(s: &str) -> String {
    s.replace("_x000D_", "").replace("_x000A_", "\n").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

fn pump_pipe<R: Read>(r: &mut R, which: u8, tx: mpsc::Sender<(u8, Vec<u8>)>) {
    let mut buf = [0u8; 8192];
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                if tx.send((which, buf[..n].to_vec())).is_err() {
                    return;
                }
            }
        }
    }
}

fn kill_tree(pid: u32) {
    let _ = util::run_cmd(
        "taskkill",
        &["/PID".into(), pid.to_string(), "/T".into(), "/F".into()],
        None,
        5_000,
    );
}

fn code_of(status: &std::process::ExitStatus) -> Option<i64> {
    status.code().map(|c| c as i64)
}

fn batch_exec(
    sock: &mut TcpStream,
    id: &Value,
    script: &str,
    timeout_s: u64,
    started: Instant,
    row: i64,
    ctx: &Arc<Ctx>,
) {
    let r = run_ps(script, timeout_s, ctx, |_, _| true);
    let dur = started.elapsed().as_millis() as i64;
    let r = match r {
        Ok(r) => r,
        Err(e) => {
            let _ = ctx.audit.log_finish(row, "exec_failed", None, "", "", Some(dur), Some(&e));
            return wire::send_json(
                sock,
                200,
                &wire::rpc_result(id, wire::tool_text_result(&format!("{EXEC_PREFIX} {e}{PAUSE_ACTION}"), true)).to_string(),
            );
        }
    };
    finish_row_and_reply(sock, id, r, dur, row, ctx);
}

/// Final result text shared by batch + stream paths.
fn result_text(r: &ExecResult) -> (String, bool) {
    if r.timed_out {
        let partial = serde_json::json!({"partial_stdout": cap(&r.stdout), "partial_stderr": cap(&r.stderr)})
            .to_string();
        (
            format!(
                "{TIMEOUT_PREFIX} command exceeded its timeout and was killed; partial output attached — report it as INCOMPLETE.{PAUSE_ACTION}\n{partial}"
            ),
            true,
        )
    } else {
        let body = serde_json::json!({
            "exit_code": r.exit,
            "stdout": cap(&r.stdout),
            "stderr": cap(&r.stderr),
            "truncated": r.stdout.len() > MCP_OUTPUT_CAP || r.stderr.len() > MCP_OUTPUT_CAP,
        });
        // exit != 0 is NOT isError: the agent needs the stderr as data.
        (body.to_string(), false)
    }
}

fn cap(s: &str) -> String {
    if s.len() <= MCP_OUTPUT_CAP {
        return s.to_string();
    }
    let (cut, _) = crate::psaudit::truncate_utf8(s, MCP_OUTPUT_CAP);
    format!("{cut}\n… [truncated at {MCP_OUTPUT_CAP} bytes]")
}

fn finish_row_and_reply(sock: &mut TcpStream, id: &Value, r: ExecResult, dur: i64, row: i64, ctx: &Arc<Ctx>) {
    let status = if r.timed_out {
        "timeout"
    } else {
        match r.exit {
            Some(0) => "ok",
            Some(_) => "tool_error",
            None => "exec_failed",
        }
    };
    let err_note = if r.timed_out { Some("timeout") } else { None };
    let audit_res = ctx.audit.log_finish(row, status, r.exit, &r.stdout, &r.stderr, Some(dur), err_note);
    let (text, is_err) = result_text(&r);
    if let Err(e) = audit_res {
        // The command ran but the terminal row failed: report LOF_FAILED.
        return wire::send_json(
            sock,
            200,
            &wire::rpc_result(id, wire::tool_text_result(&format!("{LOG_PREFIX} terminal audit write failed: {e}{PAUSE_ACTION}"), true)).to_string(),
        );
    }
    wire::send_json(sock, 200, &wire::rpc_result(id, wire::tool_text_result(&text, is_err)).to_string());
}

/// SSE path: stream-open → throttled progress frames → final result frame.
fn stream_exec(
    sock: TcpStream,
    id: &Value,
    script: &str,
    timeout_s: u64,
    started: Instant,
    row: i64,
    progress_token: Option<&Value>,
    ctx: &Arc<Ctx>,
) {
    let Some(mut sse) = wire::SseStream::start(sock) else {
        let _ = ctx.audit.log_finish(row, "client_disconnected", None, "", "", Some(0), Some("connect lost before headers"));
        return;
    };
    let token = progress_token.cloned().unwrap_or(Value::Null);
    let have_token = progress_token.is_some();
    let mut produced: u64 = 0;
    let mut tail = String::new();
    let mut last_frame = Instant::now() - Duration::from_secs(60);
    let mut pending_bytes = 0usize;
    let mut dead = false;
    let r = run_ps(script, timeout_s, ctx, |which, chunk| {
        if dead {
            return false;
        }
        produced += chunk.len() as u64;
        pending_bytes += chunk.len();
        let piece = String::from_utf8_lossy(chunk).into_owned();
        tail.push_str(&piece);
        if tail.len() > 4096 {
            tail = tail[tail.len() - 2048..].to_string();
        }
        // Throttle: every 2 s OR once ≥8 KB accumulated (first wins).
        if have_token && (last_frame.elapsed() >= Duration::from_secs(2) || pending_bytes >= 8192) {
            let f = wire::progress_frame(&token, produced, &tail, if which == "o" { &piece } else { "" }, if which == "e" { &piece } else { "" });
            sse.message(&f);
            sse.flush();
            last_frame = Instant::now();
            pending_bytes = 0;
        }
        if sse.broken() {
            dead = true;
            return false;
        }
        true
    });
    let dur = started.elapsed().as_millis() as i64;
    let mut r = match r {
        Ok(r) => r,
        Err(e) => {
            let f = wire::rpc_result(id, wire::tool_text_result(&format!("{EXEC_PREFIX} {e}{PAUSE_ACTION}"), true));
            sse.message(&f);
            sse.finish();
            let _ = ctx.audit.log_finish(row, "exec_failed", None, "", "", Some(dur), Some(&e));
            return;
        }
    };
    if dead {
        // Client vanished mid-stream: subprocess already killed by run_ps's
        // deadline? Not necessarily — kill explicitly, store partial output.
        r.disconnected = true;
        let _ = ctx.audit.log_finish(row, "client_disconnected", r.exit, &r.stdout, &r.stderr, Some(dur), Some("client disconnected mid-stream"));
        return;
    }
    let (text, is_err) = result_text(&r);
    let status = if r.timed_out {
        "timeout"
    } else {
        match r.exit {
            Some(0) => "ok",
            Some(_) => "tool_error",
            None => "exec_failed",
        }
    };
    // 补发规则：final result frame 必须在关流之前。
    let f = wire::rpc_result(id, wire::tool_text_result(&text, is_err));
    sse.message(&f);
    sse.flush();
    sse.finish();
    let _ = ctx.audit.log_finish(row, status, r.exit, &r.stdout, &r.stderr, Some(dur), if r.timed_out { Some("timeout") } else { None });
}

// ── auth ──────────────────────────────────────────────────────────

fn bearer(header: Option<&str>) -> String {
    header
        .unwrap_or("")
        .trim()
        .strip_prefix("Bearer ")
        .or_else(|| header.unwrap_or("").trim().strip_prefix("bearer "))
        .unwrap_or("")
        .trim()
        .to_string()
}

fn bearer_fp(header: Option<&str>) -> String {
    let t = bearer(header);
    if t.is_empty() {
        return "<none>".into();
    }
    format!("{}…", t.chars().take(6).collect::<String>())
}

fn check_auth(header: Option<&str>) -> bool {
    let got = bearer(header);
    if got.is_empty() {
        return false;
    }
    match host_ps_key() {
        Some(want) if !want.is_empty() => got == want,
        _ => false, // unset/rotated-away variable = everything 401 (by design)
    }
}

/// Live read of HKCU\Environment\HOST_PS_KEY — registry is the source of
/// truth so rotation is one `setx` away with zero restarts.
#[cfg(windows)]
pub fn host_ps_key() -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_NONE,
    };
    unsafe {
        let name: Vec<u16> = "Environment\0".encode_utf16().collect();
        let mut sub = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(name.as_ptr()), 0, KEY_READ, &mut sub)
            .ok()
            .ok()?;
        let val: Vec<u16> = "HOST_PS_KEY\0".encode_utf16().collect();
        let mut size = 0u32;
        let mut typ = REG_NONE;
        if RegQueryValueExW(sub, PCWSTR(val.as_ptr()), None, Some(&mut typ), None, Some(&mut size))
            .is_err()
        {
            let _ = RegCloseKey(sub);
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let got = RegQueryValueExW(
            sub,
            PCWSTR(val.as_ptr()),
            None,
            Some(&mut typ),
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut size),
        );
        let _ = RegCloseKey(sub);
        if got.is_err() {
            return None;
        }
        let n = (size as usize / 2).min(buf.len());
        // REG_SZ carries its terminating NUL; String::trim does not strip it.
        Some(String::from_utf16_lossy(&buf[..n]).trim_matches('\0').trim().to_string())
    }
}

#[cfg(not(windows))]
pub fn host_ps_key() -> Option<String> {
    None
}

// ── tools ─────────────────────────────────────────────────────────

pub fn tools_spec() -> Value {
    serde_json::json!([
        {
            "name": "powershell",
            "description": format!("Execute a PowerShell script on the Windows HOST and return stdout/stderr/exit_code. If this call fails with a [HOST_PS_GATEWAY_*] error, follow the AGENTS.md rules: [HOST_PS_GATEWAY_DISABLED] means the user switched host PowerShell OFF — treat it as no permission, keep working locally, do NOT pause or report an incident; any other prefix is a connection fault — pause this task and notify the user."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "PowerShell script text"},
                    "timeout_seconds": {"type": "integer", "default": 120, "maximum": 600}
                },
                "required": ["script"]
            }
        },
        {
            "name": "health",
            "description": "Connectivity probe. disabled:true means the user switched host PowerShell OFF in Pi Supervisor (no permission — NOT a fault, do NOT pause); an unreachable/failed call is a connection fault per AGENTS.md.",
            "inputSchema": {"type": "object", "properties": {}, "required": []}
        }
    ])
}

fn health_result(ctx: &Ctx, disabled: bool) -> Value {
    let db_ok = ctx
        .audit
        .conn
        .lock()
        .map(|c| c.execute_batch("SELECT 1").is_ok())
        .unwrap_or(false);
    serde_json::json!({
        "host_name": std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into()),
        "ps_version": ps_version(),
        "uptime_seconds": ctx.started.elapsed().as_secs(),
        "db_ok": db_ok,
        "sse_ok": ctx.sse_mode != "off",
        "disabled": disabled || !ctx.enabled.load(Ordering::Relaxed),
    })
}

/// `powershell -Command $PSVersionTable` once, cached (5 s cap).
fn ps_version() -> String {
    use std::sync::OnceLock;
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| {
        util::run_cmd(
            "powershell",
            &[
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "$PSVersionTable.PSVersion.ToString()".into(),
            ],
            None,
            8_000,
        )
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "5.1".into())
    })
    .clone()
}

// ── misc helpers ──────────────────────────────────────────────────

static SEQ: AtomicU64 = AtomicU64::new(0);
fn new_request_id() -> String {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{:08x}-{:x}-{:x}", std::process::id(), nanos, n)
}

/// base64(UTF-16LE) for -EncodedCommand, std-only.
fn b64_utf16le(s: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = s
        .encode_utf16()
        .flat_map(|u| [u as u8, (u >> 8) as u8])
        .collect();
    let mut out = String::new();
    for ch in bytes.chunks(3) {
        let b = [ch[0], ch.get(1).copied().unwrap_or(0), ch.get(2).copied().unwrap_or(0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if ch.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if ch.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn registry_read_matches_user_env() {
        // Diagnostic + regression: the gateway's live registry read must see
        // the same value [Environment]::SetEnvironmentVariable(...,'User')
        // writes (HKCU\Environment REG_SZ).
        let want = util::run_cmd(
            "powershell",
            &[
                "-NoProfile".into(),
                "-Command".into(),
                "[Environment]::GetEnvironmentVariable('HOST_PS_KEY','User')".into(),
            ],
            None,
            8_000,
        )
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
        match want {
            Some(w) => {
                let got = host_ps_key();
                assert_eq!(got.as_deref(), Some(w.as_str()), "registry read != user env");
            }
            None => eprintln!("HOST_PS_KEY not in probe env — skipped"),
        }
    }

    #[test]
    fn strip_clixml_keeps_real_errors() {
        let noisy = "#< CLIXML\r\n<Objs Version=\"1.1.0.1\" xmlns=\"http://schemas.microsoft.com/powershell/2004/04\"><Obj S=\"progress\" RefId=\"0\"><TN RefId=\"0\"><T>System.Management.Automation.PSCustomObject</T></TN></Obj></Objs>AccessDenied: C:\\Windows\\Temp\\x";
        assert_eq!(strip_clixml(noisy), "AccessDenied: C:\\Windows\\Temp\\x");
        assert_eq!(strip_clixml("plain stderr"), "plain stderr");
        assert_eq!(strip_clixml(""), "");
        // Write-Error records live INSIDE the blob and must be recovered.
        let errblob = "#< CLIXML\r\n<Objs Version=\"1.1.0.1\" xmlns=\"http://schemas.microsoft.com/powershell/2004/04\"><S S=\"Error\">demo_err : demo _x000D__x000A_</S><S S=\"Error\">    + CategoryInfo: NotSpecified _x000D__x000A_</S></Objs>";
        let got = strip_clixml(errblob);
        assert!(got.contains("demo_err : demo"), "{got}");
        assert!(got.contains("+ CategoryInfo: NotSpecified"), "{got}");
        assert!(!got.contains("<Objs"), "no XML left");
    }

    #[test]
    fn b64_matches_powershell_encoding() {
        // Known vector: "Get-Date" in UTF-16LE base64 (as powershell expects).
        assert_eq!(b64_utf16le("Get-Date"), "RwBlAHQALQBEAGEAdABlAA==");
        assert_eq!(b64_utf16le(""), "");
        assert_eq!(b64_utf16le("é"), "6QA="); // U+00E9 → UTF-16LE E9 00
    }

    #[test]
    fn bearer_parsing_tolerates_shape_noise() {
        assert_eq!(bearer(Some("Bearer abc123")), "abc123");
        assert_eq!(bearer(Some("bearer abc123 ")), "abc123");
        assert_eq!(bearer(Some("Basic zzz")), "");
        assert_eq!(bearer(None), "");
        assert_eq!(bearer_fp(Some("Bearer sbx-cs-UY2FZ3")), "sbx-cs…");
    }

    #[test]
    fn disabled_text_is_class_b() {
        // The whole point of r4.1: DISABLED must NOT carry the pause clause.
        assert!(DISABLED_TEXT.contains("do NOT pause"));
        assert!(!DISABLED_TEXT.contains("pause this task"));
        assert!(PAUSE_ACTION.contains("pause this task"));
    }

    #[test]
    fn result_text_caps_output_and_marks_partial() {
        let big = "x".repeat(MCP_OUTPUT_CAP + 10);
        let r = ExecResult { stdout: big.clone(), stderr: String::new(), exit: Some(0), timed_out: false, disconnected: false };
        let (text, is_err) = result_text(&r);
        assert!(!is_err);
        assert!(text.contains("truncated"));
        let t = ExecResult { stdout: "partial".into(), stderr: String::new(), exit: None, timed_out: true, disconnected: false };
        let (text, is_err) = result_text(&t);
        assert!(is_err && text.contains(TIMEOUT_PREFIX) && text.contains("partial"));
    }
}
