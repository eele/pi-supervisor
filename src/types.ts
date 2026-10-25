/** Service identifiers known to the supervisor. */
export type ServiceId = "pageant" | "bridge" | "notify";

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

export type TabId = "sessions" | "services" | "sandbox";
