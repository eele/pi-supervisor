//! Host PowerShell audit store — ~/.pi-su/ps-audit.db, table ps_op_log.
//!
//! Every JSON-RPC request the hostps gateway sees lands exactly one row
//! (auth failures included; the token itself is never stored, only a
//! 6-char fingerprint inside `error`). tools/call rows are written
//! start→finish (two-phase) so even a gateway crash leaves a `started`
//! trace. A failed write is a HARD failure: the gateway must refuse to
//! execute ([HOST_PS_GATEWAY_LOG_FAILED]).
//!
//! The PS Audit UI tab reads the same Database handle in-process (WAL);
//! writes serialize through the single Mutex<Connection>.

use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

pub const AUDIT_MAX_STDOUT: usize = 1_000_000;
pub const AUDIT_MAX_STDERR: usize = 262_144;

pub fn db_path() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".pi-su").join("ps-audit.db"))
}

pub struct PsAudit {
    pub conn: Mutex<Connection>,
}

/// UTC ISO8601 `YYYY-MM-DDTHH:MM:SSZ` without chrono (seconds since epoch →
/// Howard Hinnant's civil_from_days; the whole app only ever needs UTC).
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, tod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (h, mi, s) = (tod / 3600, (tod / 60) % 60, tod % 60);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

impl PsAudit {
    /// Open (creating the dir/db/table idempotently) with WAL + NORMAL sync,
    /// and run the startup retention pass.
    pub fn open() -> Result<std::sync::Arc<PsAudit>, String> {
        let path = db_path().ok_or("USERPROFILE is not set")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let conn = Connection::open(&path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL").map_err(|e| e.to_string())?;
        conn.pragma_update(None, "synchronous", "NORMAL").map_err(|e| e.to_string())?;
        conn.execute_batch(
            r#"
CREATE TABLE IF NOT EXISTS ps_op_log (
  id          INTEGER PRIMARY KEY,
  ts          TEXT NOT NULL,
  request_id  TEXT,
  client      TEXT,
  method      TEXT NOT NULL,
  tool        TEXT,
  args_json   TEXT,
  exit_code   INTEGER,
  stdout      TEXT,
  stdout_truncated INTEGER DEFAULT 0,
  stderr      TEXT,
  duration_ms INTEGER,
  stream      INTEGER DEFAULT 0,
  status      TEXT NOT NULL,
  error       TEXT
);
CREATE INDEX IF NOT EXISTS idx_psop_ts    ON ps_op_log(ts);
CREATE INDEX IF NOT EXISTS idx_psop_tool  ON ps_op_log(tool);
CREATE INDEX IF NOT EXISTS idx_psop_stat  ON ps_op_log(status);
"#,
        )
        .map_err(|e| format!("cannot create ps_op_log: {e}"))?;
        let this = std::sync::Arc::new(PsAudit { conn: Mutex::new(conn) });
        let _ = this.purge();
        Ok(this)
    }

    /// tools/call phase 1:抢占式 started 行，返回 rowid。写失败 = 调用方必须拒绝执行。
    pub fn log_start(&self, request_id: &str, client: &str, tool: &str, args_json: &str, stream: bool) -> Result<i64, String> {
        let c = self.conn.lock().map_err(|e| e.to_string())?;
        c.execute(
            "INSERT INTO ps_op_log (ts, request_id, client, method, tool, args_json, stream, status)
             VALUES (?1, ?2, ?3, 'tools-call', ?4, ?5, ?6, 'started')",
            params![now_iso(), request_id, client, tool, args_json, stream as i64],
        )
        .map(|_| c.last_insert_rowid())
        .map_err(|e| e.to_string())
    }

    /// tools/call phase 2: 终态 UPDATE。stdout/stderr 截断入库并置标记。
    pub fn log_finish(
        &self,
        id: i64,
        status: &str,
        exit_code: Option<i64>,
        stdout: &str,
        stderr: &str,
        duration_ms: Option<i64>,
        error: Option<&str>,
    ) -> Result<(), String> {
        let (out, out_trunc) = truncate_utf8(stdout, AUDIT_MAX_STDOUT);
        let (err, _) = truncate_utf8(stderr, AUDIT_MAX_STDERR);
        let c = self.conn.lock().map_err(|e| e.to_string())?;
        c.execute(
            "UPDATE ps_op_log SET status=?2, exit_code=?3, stdout=?4, stdout_truncated=?5,
             stderr=?6, duration_ms=?7, error=?8 WHERE id=?1",
            params![id, status, exit_code, out, out_trunc as i64, err, duration_ms, error],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    /// 一次性终态行：auth_failed / ping / tools-list / disabled 等。
    pub fn log_simple(
        &self,
        request_id: &str,
        client: &str,
        method: &str,
        tool: Option<&str>,
        args_json: Option<&str>,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), String> {
        let c = self.conn.lock().map_err(|e| e.to_string())?;
        c.execute(
            "INSERT INTO ps_op_log (ts, request_id, client, method, tool, args_json, status, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![now_iso(), request_id, client, method, tool, args_json, status, error],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    /// 分页 + 三域 LIKE 搜索（args_json / stdout / stderr），ts DESC, id DESC。
    pub fn query(&self, q: &str, page: u32, page_size: u32) -> Result<(i64, Vec<PsOpRow>), String> {
        let page_size = page_size.clamp(1, 200);
        let page = page.max(1);
        let pat = format!("%{}%", escape_like(q));
        let empty = q.trim().is_empty();
        let c = self.conn.lock().map_err(|e| e.to_string())?;
        let where_sql = if empty {
            ""
        } else {
            " WHERE (args_json LIKE ?1 ESCAPE '\\' OR stdout LIKE ?1 ESCAPE '\\' OR stderr LIKE ?1 ESCAPE '\\')"
        };
        let count_sql = format!("SELECT COUNT(*) FROM ps_op_log{where_sql}");
        let total: i64 = if empty {
            c.query_row(&count_sql, [], |r| r.get(0))
        } else {
            c.query_row(&count_sql, params![pat], |r| r.get(0))
        }
        .map_err(|e| e.to_string())?;
        // With the filter present ?1 is taken by the LIKE pattern; renumber
        // LIMIT/OFFSET accordingly for the param-less (empty query) variant.
        let (li, oi) = if empty { (1, 2) } else { (2, 3) };
        let mut stmt = c
            .prepare(&format!(
                "SELECT id, ts, status, exit_code, duration_ms, tool, stream,
                        stdout_truncated, error, args_json
                 FROM ps_op_log{where_sql}
                 ORDER BY ts DESC, id DESC LIMIT ?{li} OFFSET ?{oi}"
            ))
            .map_err(|e| e.to_string())?;
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if !empty {
            binds.push(Box::new(pat));
        }
        binds.push(Box::new(page_size as i64));
        binds.push(Box::new((page - 1) as i64 * page_size as i64));
        let rows = stmt
            .query_map(rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())), |r| {
                let args: Option<String> = r.get(9)?;
                Ok(PsOpRow {
                    id: r.get(0)?,
                    ts: r.get(1)?,
                    status: r.get(2)?,
                    exit_code: r.get(3)?,
                    duration_ms: r.get(4)?,
                    tool: r.get(5)?,
                    stream: r.get::<_, i64>(6)? != 0,
                    stdout_truncated: r.get::<_, i64>(7)? != 0,
                    error: r.get(8)?,
                    preview: preview_of(args.as_deref()),
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok((total, out))
    }

    /// 展开行的三段全文（COMMAND / STDOUT / STDERR）。
    pub fn detail(&self, id: i64) -> Result<Option<PsOpFull>, String> {
        let c = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = c
            .prepare(
                "SELECT id, ts, status, exit_code, duration_ms, tool, method, stream,
                        stdout_truncated, error, args_json, stdout, stderr, request_id, client
                 FROM ps_op_log WHERE id=?1",
            )
            .map_err(|e| e.to_string())?;
        stmt.query_row(params![id], |r| {
            Ok(PsOpFull {
                id: r.get(0)?,
                ts: r.get(1)?,
                status: r.get(2)?,
                exit_code: r.get(3)?,
                duration_ms: r.get(4)?,
                tool: r.get(5)?,
                method: r.get(6)?,
                stream: r.get::<_, i64>(7)? != 0,
                stdout_truncated: r.get::<_, i64>(8)? != 0,
                error: r.get(9)?,
                command: command_of(r.get::<_, Option<String>>(10)?.as_deref()),
                stdout: r.get::<_, Option<String>>(11)?.unwrap_or_default(),
                stderr: r.get::<_, Option<String>>(12)?.unwrap_or_default(),
                request_id: r.get(13)?,
                client: r.get(14)?,
            })
        })
        .optional()
        .map_err(|e| e.to_string())
    }

    /// 保留策略：30 天清理 + 崩溃遗留 started 归入 exec_failed；>64MB 触发 VACUUM。
    pub fn purge(&self) -> Result<(), String> {
        let c = self.conn.lock().map_err(|e| e.to_string())?;
        let cutoff = {
            let secs = crate::util::now_unix() - 30 * 86_400;
            // Re-render cutoff ISO from epoch seconds via now_iso's math:
            iso_from_unix(secs)
        };
        c.execute(
            "UPDATE ps_op_log SET status='exec_failed', error=COALESCE(error,'orphaned started row (gateway crash)') WHERE status='started' AND ts < ?1",
            params![cutoff],
        )
        .map_err(|e| e.to_string())?;
        c.execute("DELETE FROM ps_op_log WHERE ts < ?1", params![cutoff]).map_err(|e| e.to_string())?;
        let pages: i64 = c
            .query_row("PRAGMA page_count", [], |r| r.get(0))
            .unwrap_or(0);
        let page_size: i64 = c
            .query_row("PRAGMA page_size", [], |r| r.get(0))
            .unwrap_or(4096);
        if pages * page_size > 64 * 1024 * 1024 {
            drop(c);
            let c2 = self.conn.lock().map_err(|e| e.to_string())?;
            let _ = c2.execute_batch("VACUUM");
        }
        Ok(())
    }
}

fn iso_from_unix(secs: i64) -> String {
    // Same civil algorithm as now_iso, fixed input.
    let now = crate::util::now_unix();
    let delta = now - secs;
    // Cheap: shift now_iso's timestamp back by whole days then re-derive —
    // instead just duplicate the algorithm here for exactness.
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let _ = delta;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (h, mi, s) = (tod / 3600, (tod / 60) % 60, tod % 60);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// UTF-8 safe truncation at a byte cap (never splits a char).
pub fn truncate_utf8(s: &str, cap: usize) -> (String, bool) {
    if s.len() <= cap {
        return (s.to_string(), false);
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

/// LIKE 通配符转义（配合 ESCAPE '\'）。
fn escape_like(q: &str) -> String {
    let mut out = String::with_capacity(q.len());
    for ch in q.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn command_of(args_json: Option<&str>) -> String {
    args_json
        .and_then(|a| serde_json::from_str::<serde_json::Value>(a).ok())
        .and_then(|v| v.get("script").and_then(|s| s.as_str()).map(String::from))
        .unwrap_or_else(|| args_json.unwrap_or_default().to_string())
}

fn preview_of(args_json: Option<&str>) -> String {
    let cmd = command_of(args_json);
    let first = cmd.lines().next().unwrap_or("").trim();
    if first.chars().count() > 200 {
        format!("{}…", first.chars().take(200).collect::<String>())
    } else {
        first.to_string()
    }
}

#[derive(serde::Serialize)]
pub struct PsOpRow {
    pub id: i64,
    pub ts: String,
    pub status: String,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<i64>,
    pub tool: Option<String>,
    pub stream: bool,
    pub stdout_truncated: bool,
    pub error: Option<String>,
    pub preview: String,
}

#[derive(serde::Serialize)]
pub struct PsOpFull {
    pub id: i64,
    pub ts: String,
    pub status: String,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<i64>,
    pub tool: Option<String>,
    pub method: String,
    pub stream: bool,
    pub stdout_truncated: bool,
    pub error: Option<String>,
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub request_id: Option<String>,
    pub client: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_audit(tag: &str) -> std::sync::Arc<PsAudit> {
        // Isolated db file per test (USERPROFILE is real; namespaced name).
        let dir = std::env::temp_dir().join(format!("pi-su-test-{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        for ext in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(dir.join(format!("t.db{ext}")));
        }
        let conn = Connection::open(dir.join("t.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ps_op_log (
               id INTEGER PRIMARY KEY, ts TEXT NOT NULL, request_id TEXT, client TEXT,
               method TEXT NOT NULL, tool TEXT, args_json TEXT, exit_code INTEGER,
               stdout TEXT, stdout_truncated INTEGER DEFAULT 0, stderr TEXT,
               duration_ms INTEGER, stream INTEGER DEFAULT 0, status TEXT NOT NULL, error TEXT);",
        )
        .unwrap();
        std::sync::Arc::new(PsAudit { conn: Mutex::new(conn) })
    }

    #[test]
    fn round_trip_and_query_desc() {
        let a = temp_audit("rt");
        let id = a.log_start("r1", "1.2.3.4", "powershell", r#"{"script":"Get-Date"}"#, false).unwrap();
        a.log_finish(id, "ok", Some(0), "2026-10-26", "", Some(12), None).unwrap();
        a.log_simple("r2", "5.6.7.8", "auth", None, None, "auth_failed", Some("[HOST_PS_GATEWAY_AUTH] fp deadbe")).unwrap();
        let (total, rows) = a.query("", 1, 50).unwrap();
        assert_eq!(total, 2);
        // auth_failed 后插入 → ts 同秒也必须排最前（id DESC tie-break）
        assert_eq!(rows[0].status, "auth_failed");
        assert_eq!(rows[1].preview, "Get-Date");
    }

    #[test]
    fn search_covers_command_stdout_stderr() {
        let a = temp_audit("search");
        let id = a.log_start("r", "c", "powershell", r#"{"script":"Get-Process | Out-Null"}"#, false).unwrap();
        a.log_finish(id, "ok", Some(0), "chrome.exe pid=123", "warn: nope", Some(3), None).unwrap();
        assert_eq!(a.query("Get-Process", 1, 50).unwrap().0, 1, "command hit");
        assert_eq!(a.query("chrome", 1, 50).unwrap().0, 1, "stdout hit");
        assert_eq!(a.query("nope", 1, 50).unwrap().0, 1, "stderr hit");
        assert_eq!(a.query("nothing%", 1, 50).unwrap().0, 0, "wildcards escaped");
    }

    #[test]
    fn truncation_flags_round_trip() {
        let (s, t) = truncate_utf8("héllo", 3);
        assert_eq!(s, "hé");
        assert!(t);
        let (s2, t2) = truncate_utf8("abc", 10);
        assert_eq!(s2, "abc");
        assert!(!t2);
    }

    #[test]
    fn now_iso_shape() {
        let s = now_iso();
        assert_eq!(s.len(), 20, "{s}");
        assert!(s.ends_with('Z'));
        assert_eq!(&s[4..5], "-");
    }
}
