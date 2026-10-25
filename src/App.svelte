<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import TitleBar from "./components/TitleBar.svelte";
  import TabBar from "./components/TabBar.svelte";
  import SessionsPanel from "./components/SessionsPanel.svelte";
  import ServiceRow from "./components/ServiceRow.svelte";
  import SandboxCard from "./components/SandboxCard.svelte";
  import ToggleSwitch from "./components/ToggleSwitch.svelte";
  import ConfirmDialog from "./components/ConfirmDialog.svelte";
  import ContextMenu from "./components/ContextMenu.svelte";
  import {
    attachSession,
    disableLegacyStartup,
    getAutostart,
    getConfigCorrupt,
    getRecreateState,
    getSandbox,
    getServices,
    getSessions,
    pushTokens,
    recreateSandbox,
    setAutostart,
    startService,
    stopService,
  } from "./api";
  import type {
    RecreateState,
    SandboxInfo,
    ServiceId,
    ServiceInfo,
    SessionsSnapshot,
    TabId,
  } from "./types";

  // ── Tabs ───────────────────────────────────────────────────
  const TABS: { id: TabId; label: string }[] = [
    { id: "sessions", label: "Sessions" },
    { id: "services", label: "Background Services" },
    { id: "sandbox", label: "Sandbox" },
  ];
  let activeTab = $state<TabId>("sessions");
  const activeIndex = $derived(
    Math.max(
      0,
      TABS.findIndex((t) => t.id === activeTab)
    )
  );

  // ── Polled data ────────────────────────────────────────────
  let services = $state<ServiceInfo[]>([]);
  let sandbox = $state<SandboxInfo | null>(null);
  let sessions = $state<SessionsSnapshot>({
    has_data: false,
    poll_error: "",
    sessions: [],
  });
  let recreate = $state<RecreateState>({ active: false, steps: [], error: null });
  let recreateRunning = $state(false);
  let maximized = $state(false);

  // Poll failure flags (statusbar only; polling always keeps running).
  let servicesError = $state(false);
  let sandboxError = $state(false);
  let sessionsError = $state(false);

  // Startup toggle
  let autostart = $state(false);
  let startupBusy = $state(false);
  let startupError = $state("");

  // Sandbox card busy flags
  let refreshing = $state(false);
  let pushing = $state(false);

  // Config corruption is a startup fact: read once, warn forever after.
  let configCorrupt = $state(false);

  // Transient statusbar message (Copy status / logs / legacy-disable result).
  // Expires on a 5 s timer — must NOT be wiped by the next background poll,
  // or button feedback becomes unreadable with 2 s service polling.
  let flashMsg = $state<string | null>(null);
  let flashIsError = $state(false);
  let flashUntil = 0;

  // Dialogs / menu
  let recreateConfirmVisible = $state(false);
  let legacyConfirmVisible = $state(false);
  let ctxVisible = $state(false);
  let ctxX = $state(0);
  let ctxY = $state(0);
  let ctxItems = $state<{ label: string; onClick: () => void }[]>([]);

  function errText(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
  }
  function flash(text: string, isError = false) {
    flashUntil = Date.now() + 5_000;
    flashMsg = text;
    flashIsError = isError;
  }

  const RECREATE_STEP_LABELS = [
    "rm",
    "resolve latest kit",
    "run -d",
    "wait ready",
    "push tokens",
  ];

  // ── Statusbar composition ──────────────────────────────────
  const liveCount = $derived(
    sessions.has_data ? sessions.sessions.filter((r) => r.live).length : 0
  );

  const statusText = $derived.by(() => {
    if (startupBusy) return "Setting startup…";
    if (startupError) return startupError;
    if (flashMsg !== null) return flashMsg;
    const parts: string[] = [];
    if (services.length > 0) {
      const n = services.filter((s) => s.state === "running").length;
      parts.push(
        n === services.length
          ? "All services healthy"
          : `${n} of ${services.length} services running`
      );
    }
    if (sandbox) {
      parts.push(
        sandbox.state === "not-created"
          ? "pi-ai not created"
          : `pi-ai ${sandbox.state}`
      );
    }
    parts.push(`${liveCount} live sessions`);
    parts.push("refresh 3 s");
    let s = parts.join(" · ");
    const errs: string[] = [];
    if (configCorrupt) errs.push("config.json corrupt — defaults in use, check the file");
    if (servicesError) errs.push("services poll failed");
    if (sandboxError) errs.push("sandbox poll failed");
    if (sessionsError) errs.push("sessions poll failed");
    else if (sessions.poll_error) errs.push(sessions.poll_error);
    if (errs.length > 0) s += " · " + errs.join(", ");
    return s;
  });

  const statusIsError = $derived(
    startupError !== "" ||
      (flashMsg !== null && flashIsError) ||
      configCorrupt ||
      servicesError ||
      sandboxError ||
      sessionsError ||
      sessions.poll_error !== ""
  );

  // ── Polling (setInterval inside $effect, cleanup on teardown) ──
  $effect(() => {
    let dead = false;
    const poll = async () => {
      try {
        const r = await getServices();
        if (dead) return;
        services = r;
        servicesError = false;
        startupError = "";
        if (flashMsg !== null && Date.now() > flashUntil) flashMsg = null;
      } catch {
        if (!dead) servicesError = true;
      }
      try {
        const r = await getSandbox();
        if (dead) return;
        sandbox = r;
        sandboxError = false;
      } catch {
        if (!dead) sandboxError = true;
      }
    };
    poll();
    const t = setInterval(poll, 2000);
    return () => {
      dead = true;
      clearInterval(t);
    };
  });

  $effect(() => {
    let dead = false;
    const poll = async () => {
      try {
        const r = await getSessions();
        if (dead) return;
        sessions = r;
        sessionsError = false;
      } catch {
        if (!dead) sessionsError = true;
      }
    };
    poll();
    const t = setInterval(poll, 3000);
    return () => {
      dead = true;
      clearInterval(t);
    };
  });

  $effect(() => {
    let dead = false;
    getConfigCorrupt()
      .then((v) => {
        if (!dead) configCorrupt = v;
      })
      .catch(() => {});
    return () => {
      dead = true;
    };
  });

  // Autostart truth = HKCU Run key: read once at startup.
  $effect(() => {
    let dead = false;
    getAutostart()
      .then((v) => {
        if (!dead) autostart = v;
      })
      .catch(() => {});
    return () => {
      dead = true;
    };
  });

  // Maximized state: backend emits "pi-maximized" (bool payload). In the
  // browser mock the listener simply never fires — swallow all errors.
  $effect(() => {
    let unlisten: (() => void) | undefined;
    let dead = false;
    try {
      listen("pi-maximized", (e) => {
        if (!dead) maximized = e.payload as boolean;
      })
        .then((f) => {
          if (dead) f();
          else unlisten = f;
        })
        .catch(() => {});
    } catch {
      /* not running under Tauri */
    }
    return () => {
      dead = true;
      unlisten?.();
    };
  });

  // While a recreate is running, poll its step ticker at 800 ms; on
  // completion, immediately re-read the (new) sandbox + services.
  $effect(() => {
    if (!recreateRunning) return;
    let dead = false;
    let timer: ReturnType<typeof setInterval>;
    const poll = async () => {
      try {
        const st = await getRecreateState();
        if (dead) return;
        recreate = st;
        if (!st.active) {
          recreateRunning = false;
          clearInterval(timer);
          flash("Recreate complete.", false);
          try {
            sandbox = await getSandbox();
          } catch {
            /* keep stale copy */
          }
          try {
            services = await getServices();
          } catch {
            /* keep stale copy */
          }
        }
      } catch {
        /* ticker poll failures are non-fatal */
      }
    };
    poll();
    timer = setInterval(poll, 800);
    return () => {
      dead = true;
      clearInterval(timer);
    };
  });

  // ── Actions ────────────────────────────────────────────────
  async function onToggleService(id: ServiceId) {
    const info = services.find((s) => s.id === id);
    try {
      if (info && info.state === "running") await stopService(id);
      else await startService(id);
      services = await getServices();
      flashMsg = null;
    } catch (e) {
      flash(`Failed to toggle "${info?.name ?? id}": ${errText(e)}`, true);
    }
  }

  async function onAttach(name: string) {
    try {
      await attachSession(name);
    } catch (e) {
      flash(`Attach failed: ${errText(e)}`, true);
    }
  }

  async function onRefreshSandbox() {
    refreshing = true;
    try {
      sandbox = await getSandbox();
      flash("Sandbox state refreshed.", false);
    } catch (e) {
      flash(`Refresh failed: ${errText(e)}`, true);
    } finally {
      refreshing = false;
    }
  }

  async function onPushTokens() {
    pushing = true;
    try {
      const msg = await pushTokens();
      flash(msg, false);
    } catch (e) {
      flash(`Push tokens failed: ${errText(e)}`, true);
    } finally {
      pushing = false;
    }
  }

  function openRecreateConfirm() {
    recreateConfirmVisible = true;
  }

  async function onRecreateConfirmed() {
    recreateConfirmVisible = false;
    try {
      await recreateSandbox();
      // Prime the ticker immediately (real steps arrive on the 800 ms poll).
      recreate = {
        active: true,
        steps: RECREATE_STEP_LABELS.map((label, i) => ({
          label,
          status: i === 0 ? ("active" as const) : ("pending" as const),
        })),
        error: null,
      };
      recreateRunning = true;
    } catch (e) {
      flash(`Recreate failed: ${errText(e)}`, true);
    }
  }

  async function onStartupToggle(on: boolean) {
    const prev = autostart;
    startupBusy = true;
    startupError = "";
    try {
      await setAutostart(on);
      autostart = await getAutostart();
    } catch (e) {
      startupError = `Startup setting failed: ${errText(e)}`;
      try {
        autostart = await getAutostart();
      } catch {
        autostart = prev; // revert to last known state
      }
    } finally {
      startupBusy = false;
    }
  }

  async function onDisableLegacyConfirmed() {
    legacyConfirmVisible = false;
    try {
      const renamed = await disableLegacyStartup();
      flash(
        renamed.length === 1
          ? "Disabled 1 legacy Startup entry."
          : `Disabled ${renamed.length} legacy Startup entries.`,
        false
      );
    } catch (e) {
      flash(`Disable failed: ${errText(e)}`, true);
    }
  }

  // ── Statusbar context menu ─────────────────────────────────
  function onStatusContext(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    ctxX = e.clientX;
    ctxY = e.clientY;
    ctxItems = [
      {
        label: "Copy status",
        onClick: () => {
          const text = statusText;
          ctxVisible = false;
          if (navigator.clipboard?.writeText) {
            navigator.clipboard
              .writeText(text)
              .then(() => flash("Status copied to clipboard.", false))
              .catch(() => flash("Clipboard unavailable.", true));
          } else {
            flash("Clipboard unavailable.", true);
          }
        },
      },
      {
        label: "Open logs",
        // No backend log command yet — keep it simple: point at the folder.
        onClick: () => {
          ctxVisible = false;
          flash("Logs: %APPDATA%\\pi-supervisor\\logs", false);
        },
      },
      {
        label: "Disable legacy Startup",
        onClick: () => {
          ctxVisible = false;
          legacyConfirmVisible = true;
        },
      },
    ];
    ctxVisible = true;
  }

  function hideContextMenu() {
    ctxVisible = false;
  }
</script>

<svelte:window on:click={hideContextMenu} on:contextmenu={hideContextMenu} />

<div id="root" class:maximized data-testid="app-root">
  <TitleBar />

  <div class="window-body">
    <div class="tab-card">
      <TabBar tabs={TABS} active={activeTab} onSelect={(id) => (activeTab = id as TabId)} />
      <div class="tab-content">
        <div class="tab-track" style="transform: translateX(-{activeIndex * (100 / 3)}%)">
          <div class="tab-panel" role="tabpanel" aria-label="Sessions">
            <SessionsPanel
              snapshot={sessions}
              sandboxState={sandbox ? sandbox.state : "detached"}
              onAttach={onAttach}
            />
          </div>
          <div class="tab-panel" role="tabpanel" aria-label="Background Services">
            <div class="service-wrap">
              <div class="service-list" data-testid="service-list">
                {#each services as svc (svc.id)}
                  <ServiceRow info={svc} onToggle={onToggleService} />
                {/each}
              </div>
              <div class="page-note">
                start/stop intent is persisted to config.json — a manually stopped
                service stays Stopped across restarts
              </div>
            </div>
          </div>
          <div class="tab-panel" role="tabpanel" aria-label="Sandbox">
            {#if sandbox}
              <SandboxCard
                sandbox={sandbox}
                recreate={recreate}
                busyRefresh={refreshing}
                busyPush={pushing}
                onRefresh={onRefreshSandbox}
                onPushTokens={onPushTokens}
                onRecreate={openRecreateConfirm}
              />
              <div class="page-note">
                name is fixed (pi-ai, no rename control) · not created → chip
                reads "not created" and Recreate takes the pure-create path
                (skips rm)
              </div>
            {:else}
              <div class="sessions-empty">Loading…</div>
            {/if}
          </div>
        </div>
      </div>
    </div>

    <!-- Startup row: fixed below the tab card, above the statusbar -->
    <div class="startup-row" data-testid="startup-row">
      <span class="startup-label">
        Startup: launch this app at logon (replaces legacy Startup entries)
      </span>
      <ToggleSwitch
        on={autostart}
        onChange={onStartupToggle}
        ariaLabel="Launch this app at logon"
        disabled={startupBusy}
        testid="startup-toggle"
      />
    </div>
  </div>

  <!-- Statusbar: direct child of #root at the very bottom -->
  <div class="status-strip" data-testid="status-strip" oncontextmenu={onStatusContext}>
    <div
      class="status-bar"
      class:error={statusIsError}
      data-testid="status"
    >{statusText}</div>
  </div>
</div>

<ContextMenu visible={ctxVisible} x={ctxX} y={ctxY} items={ctxItems} />

<ConfirmDialog
  visible={recreateConfirmVisible}
  title="Recreate Sandbox"
  question={'Recreate "pi-ai" from the latest zot kit?'}
  message={`Removed and recreated detached (sbx run -d) from ${sandbox?.kit_repo || "zot.byte.art/blackbox/pi-kit"} (newest stamp), then both runtime tokens are re-pushed automatically.`}
  confirmLabel="Recreate"
  cancelLabel="Cancel"
  danger={true}
  onConfirm={onRecreateConfirmed}
  onCancel={() => (recreateConfirmVisible = false)}
/>

<ConfirmDialog
  visible={legacyConfirmVisible}
  title="Disable Legacy Startup"
  question="Disable the legacy Startup scripts?"
  message="Renames the legacy Startup scripts (server.vbs, pageant-bootstrap.cmd, pi-obs-wake.vbs) with a .disabled suffix so they no longer double-start. Reversible."
  confirmLabel="Disable"
  cancelLabel="Cancel"
  danger={true}
  onConfirm={onDisableLegacyConfirmed}
  onCancel={() => (legacyConfirmVisible = false)}
/>

<style>
  /* Layout classes owned by App (theme.css carries the Aqua skin). */
  .service-wrap {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }

  .page-note {
    margin-top: auto;
    padding-top: 12px;
    font-size: 10px;
    color: var(--muted);
  }
</style>
