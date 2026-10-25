import { invoke } from "@tauri-apps/api/core";
import type {
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
  },
  sandbox: {
    state: "detached",
    kit: "zot.byte.art/blackbox/pi-kit:20261025.150332.857",
    mount_path: "C:\\Users\\ele\\Tea",
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
};

const RECREATE_STEPS = ["rm", "resolve latest kit", "run -d", "wait ready", "push tokens"];

/** Deterministic knobs for Playwright. */
declare global {
  interface Window {
    __mock?: {
      setSandbox(state: SandboxInfo): void;
      setSessions(s: SessionsSnapshot): void;
      setServiceState(id: ServiceId, state: string): void;
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
