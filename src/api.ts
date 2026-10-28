import { invoke } from "@tauri-apps/api/core";
import type {
  PsOpFull,
  PsOpsPage,
  PsOpRow,
  RecreateState,
  SandboxInfo,
  ServiceId,
  ServiceInfo,
  SessionsSnapshot,
} from "./types";

/**
 * API layer – talks to the Tauri Rust backend via `invoke`.
 * Falls back to an in-browser mock when running under plain `vite dev`
 * (Playwright E2E drives the mock layer; see tests/e2e).
 */

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// ── Mock state (browser-only) ───────────────────────────────────────

interface MockState {
  services: Record<ServiceId, { name: string; subtitle: string; state: string }>;
  sandbox: SandboxInfo;
  sessions: SessionsSnapshot;
  recreate: { active: boolean; tick: number; error: string | null };
  autostart: boolean;
  legacyDisabled: string[];
  psops: PsOpFull[]; // newest first
}

// Deterministic ps_op_log fixtures for the PS Audit tab (mock mode).
function mkPsOps(): PsOpFull[] {
  const row = (
    id: number,
    ts: string,
    status: string,
    tool: string | null,
    exit: number | null,
    dur: number | null,
    command: string,
    stdout: string,
    stderr: string,
    error: string | null = null,
    stream = false
  ): PsOpFull => ({
    id, ts, status, tool, exit_code: exit, duration_ms: dur, method: tool ? "tools-call" : status === "auth_failed" ? "auth" : "ping",
    stream, stdout_truncated: false, error, command, stdout, stderr,
    request_id: `req-${id.toString(16)}`, client: `172.2${id % 10}.0.${id}:5${id % 1000} pi/1.3.0`,
  });
  return [
    row(101, "2026-10-26T09:41:02Z", "ok", "powershell", 0, 1234,
      "Get-Process | Sort-Object WS -Descending | Select-Object -First 10 Name, WS",
      "chrome chrome\nclaude node\nsvchost svchost", "", null, true),
    row(100, "2026-10-26T09:38:57Z", "timeout", "powershell", null, 120000,
      "iperf3 -s -D --logfile C:\\temp\\iperf.log ; while ($true) { Start-Sleep 5 }",
      "iperf3: Server listening on 5201 port 5201 (TCP) · accepted 0 connections in 120 s",
      "⚠ partial output — command tree killed on timeout (taskkill /T /F)",
      "[HOST_PS_GATEWAY_TIMEOUT] command exceeded its timeout; partial output attached", true),
    row(99, "2026-10-26T09:30:11Z", "auth_failed", null, null, 11,
      "", "", "", "[HOST_PS_GATEWAY_AUTH] invalid token fp sbx-cs", false),
    row(98, "2026-10-26T08:57:44Z", "ok", "powershell", 0, 380, "Get-Date",
      "2026-10-26T08:57:44Z", "", null, false),
    row(97, "2026-10-26T08:12:03Z", "client_disconnected", "powershell", null, 41000,
      "cargo build --release 2>&1 | Tee-Object build.log ; git push origin main",
      "   Compiling pi v0.1.0 (12/48)", "", "client disconnected mid-stream", true),
    row(96, "2026-10-26T08:02:19Z", "exec_failed", "powershell", 1, 210,
      "Remove-Item C:\\Windows\\Temp\\x -Recurse", "", "AccessDenied: C:\\Windows\\Temp\\x",
      null, false),
    row(95, "2026-10-26T07:55:00Z", "disabled", "powershell", null, 0,
      "Invoke-WebRequest http://example.com", "", "", "[HOST_PS_GATEWAY_DISABLED]", false),
    row(94, "2026-10-26T07:54:30Z", "ok", "health", null, 2, "", "", "", null, false),
  ];
}

const MOCK: MockState = {
  services: {
    pageant: {
      name: "Pageant SSH Agent",
      subtitle: "all ~/.ssh/*.ppk loaded (top-level only) · SSH_AUTH_SOCK maintained",
      state: "running",
    },
    bridge: {
      name: "Agent TCP Bridge",
      subtitle: "tcp://0.0.0.0:60022 · token-authenticated · Pageant WM_COPYDATA",
      state: "running",
    },
    notify: {
      name: "Notify Hub",
      subtitle: "http://0.0.0.0:60000/notify · Windows toast (reminder)",
      state: "running",
    },
    hostps: {
      name: "Host PowerShell MCP",
      subtitle:
        "http://0.0.0.0:37022/mcp · Bearer (HKCU HOST_PS_KEY) · audit → ~/.pi-su/ps-audit.db",
      state: "running",
    },
  },
  sandbox: {
    state: "detached",
    kit: "zot.byte.art/blackbox/pi-kit:20261025.150332.857",
    mount_path: "C:\\Users\\ele\\Tea",
    kit_repo: "zot.byte.art/blackbox/pi-kit",
  },
  sessions: {
    has_data: true,
    poll_error: "",
    sessions: [
      { name: "socket_20261025.150210.447", live: true },
      { name: "socket_20261025.143955.208", live: true },
      { name: "socket_20261025.091612.863", live: true },
      { name: "socket_20261024.221503.102", live: false },
    ],
  },
  recreate: { active: false, tick: 0, error: null },
  autostart: true,
  legacyDisabled: [],
  psops: mkPsOps(),
};

const RECREATE_STEPS = ["rm", "resolve latest kit", "run -d", "wait ready", "push tokens"];

/** Deterministic knobs for Playwright. */
declare global {
  interface Window {
    __mock?: {
      setSandbox(state: SandboxInfo): void;
      setSessions(s: SessionsSnapshot): void;
      setServiceState(id: ServiceId, state: string): void;
      setPsOps(rows: PsOpFull[]): void;
      reset(): void;
    };
  }
}
if (typeof window !== "undefined" && !isTauri()) {
  window.__mock = {
    setSandbox: (s) => (MOCK.sandbox = s),
    setSessions: (s) => (MOCK.sessions = s),
    setServiceState: (id, state) => {
      MOCK.services[id].state = state;
    },
    setPsOps: (rows) => {
      MOCK.psops = [...rows].sort((a, b) => b.id - a.id);
    },
    reset: () => void location.reload(),
  };
}

function delay(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

// ── Commands (mirror the Tauri IPC interface) ───────────────────────

export async function getServices(): Promise<ServiceInfo[]> {
  if (isTauri()) return invoke<ServiceInfo[]>("get_services");
  return (Object.entries(MOCK.services) as [ServiceId, (typeof MOCK.services)[ServiceId]][]).map(
    ([id, s]) => ({ id, name: s.name, subtitle: s.subtitle, state: s.state as ServiceInfo["state"] })
  );
}

export async function startService(id: ServiceId): Promise<void> {
  if (isTauri()) return invoke("start_service", { id });
  await delay(150);
  MOCK.services[id].state = "running";
}

export async function stopService(id: ServiceId): Promise<void> {
  if (isTauri()) return invoke("stop_service", { id });
  await delay(150);
  MOCK.services[id].state = "stopped";
}

export async function getSandbox(): Promise<SandboxInfo> {
  if (isTauri()) return invoke<SandboxInfo>("get_sandbox");
  return { ...MOCK.sandbox };
}

export async function recreateSandbox(): Promise<void> {
  if (isTauri()) return invoke("recreate_sandbox");
  if (MOCK.recreate.active) throw new Error("recreate already running");
  MOCK.recreate = { active: true, tick: 0, error: null };
}

export async function getRecreateState(): Promise<RecreateState> {
  if (isTauri()) return invoke<RecreateState>("get_recreate_state");
  const { active, tick, error } = MOCK.recreate;
  if (active) {
    const next = tick + 1;
    MOCK.recreate.tick = next;
    if (next >= RECREATE_STEPS.length + 1) MOCK.recreate.active = false;
  }
  const t = MOCK.recreate.tick;
  const steps = RECREATE_STEPS.map((label, i) => ({
    label,
    status: t > i + 1 ? ("done" as const) : t === i + 1 ? ("active" as const) : ("pending" as const),
  }));
  return { active: MOCK.recreate.active, steps, error };
}

export async function pushTokens(): Promise<string> {
  if (isTauri()) return invoke<string>("push_tokens");
  await delay(300);
  if (MOCK.sandbox.state === "not-created") throw new Error("sandbox pi-ai is not created");
  return "push-tokens: all tokens installed in pi-ai";
}

export async function getSessions(): Promise<SessionsSnapshot> {
  if (isTauri()) return invoke<SessionsSnapshot>("get_sessions");
  return { ...MOCK.sessions, sessions: [...MOCK.sessions.sessions] };
}

export async function attachSession(name: string): Promise<void> {
  if (isTauri()) return invoke("attach_session", { name });
  await delay(100); // no-op in the mock (no Windows Terminal here)
}

export async function getAutostart(): Promise<boolean> {
  if (isTauri()) return invoke<boolean>("get_autostart");
  return MOCK.autostart;
}

/** True when config.json failed to parse at startup (defaults in use). */
export async function getConfigCorrupt(): Promise<boolean> {
  if (isTauri()) return invoke<boolean>("get_config_corrupt");
  return false;
}

export async function setAutostart(on: boolean): Promise<void> {
  if (isTauri()) return invoke("set_autostart", { on });
  MOCK.autostart = on;
}

export async function disableLegacyStartup(): Promise<string[]> {
  if (isTauri()) return invoke<string[]>("disable_legacy_startup");
  const renamed = ["pageant-bootstrap.cmd", "server.vbs", "pi-obs-wake.vbs"].map(
    (f) => `${f}.disabled`
  );
  MOCK.legacyDisabled = renamed;
  return renamed;
}

// ── PS Audit (ps_op_log) ────────────────────────────────────────────

export async function getPsOps(q: string, page: number, pageSize: number): Promise<PsOpsPage> {
  if (isTauri()) return invoke<PsOpsPage>("get_ps_ops", { q, page, pageSize });
  await delay(60);
  const needle = q.trim().toLowerCase();
  const hit = (r: PsOpFull) =>
    !needle ||
    r.command.toLowerCase().includes(needle) ||
    r.stdout.toLowerCase().includes(needle) ||
    r.stderr.toLowerCase().includes(needle);
  const rows = MOCK.psops.filter(hit);
  const start = (Math.max(1, page) - 1) * pageSize;
  return {
    total: rows.length,
    rows: rows.slice(start, start + pageSize).map((r) => ({
      id: r.id,
      ts: r.ts,
      status: r.status,
      exit_code: r.exit_code,
      duration_ms: r.duration_ms,
      tool: r.tool,
      stream: r.stream,
      stdout_truncated: r.stdout_truncated,
      error: r.error,
      preview: r.command.split("\n")[0] ?? "",
    })),
  };
}

export async function getPsOpDetail(id: number): Promise<PsOpFull | null> {
  if (isTauri()) return invoke<PsOpFull | null>("get_ps_op_detail", { id });
  await delay(40);
  return MOCK.psops.find((r) => r.id === id) ?? null;
}
