/** Service identifiers known to the supervisor. */
export type ServiceId = "pageant" | "bridge" | "notify" | "hostps";

export type ServiceState = "running" | "starting" | "stopped" | "failed";

export interface ServiceInfo {
  id: ServiceId;
  name: string;
  /** Secondary line: ports / pipes / last-crash info. */
  subtitle: string;
  state: ServiceState;
}

export type SandboxState = "detached" | "stopped" | "not-created";

export interface SandboxInfo {
  /** `detached` == sbx reports running for the (detached) sandbox. */
  state: SandboxState;
  /** e.g. `zot.byte.art/blackbox/pi-kit:20261025.150332.857` ("" when unknown). */
  kit: string;
  /** Host mount path shown as "Mount path" (e.g. `C:\\Users\\ele\\Tea`). */
  mount_path: string;
  /** Configured kit repo without tag (config.json `zot_repo`). */
  kit_repo: string;
}

export type StepStatus = "pending" | "active" | "done" | "error";

export interface RecreateStep {
  label: string;
  status: StepStatus;
}

export interface RecreateState {
  active: boolean;
  steps: RecreateStep[];
  error: string | null;
}

export interface SessionRow {
  /** Full socket name, e.g. `socket_20261025.150210.447`. */
  name: string;
  live: boolean;
}

export interface SessionsSnapshot {
  /** False until the first 3 s poll has completed. */
  has_data: boolean;
  /** Latest parse error ("" = ok). Polling keeps running regardless. */
  poll_error: string;
  /** Newest first. */
  sessions: SessionRow[];
}

export type TabId = "sessions" | "services" | "sandbox" | "psops";

/** One ps_op_log row (list view — no big fields; expand fetches detail). */
export interface PsOpRow {
  id: number;
  /** UTC ISO8601, e.g. 2026-10-26T09:41:02Z */
  ts: string;
  /** ok | tool_error | exec_failed | timeout | auth_failed | log_failed | client_disconnected | disabled | started */
  status: string;
  exit_code: number | null;
  duration_ms: number | null;
  tool: string | null;
  stream: boolean;
  stdout_truncated: boolean;
  error: string | null;
  /** first line of the command, ≤200 chars */
  preview: string;
}

/** Expanded row: three full-text sections + meta. */
export interface PsOpFull {
  id: number;
  ts: string;
  status: string;
  exit_code: number | null;
  duration_ms: number | null;
  tool: string | null;
  method: string;
  stream: boolean;
  stdout_truncated: boolean;
  error: string | null;
  command: string;
  stdout: string;
  stderr: string;
  request_id: string | null;
  client: string | null;
}

export interface PsOpsPage {
  total: number;
  rows: PsOpRow[];
}
