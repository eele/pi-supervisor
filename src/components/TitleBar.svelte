<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";

  interface Props {
    title?: string;
  }
  let { title = "Pi Supervisor" }: Props = $props();

  function isTauri(): boolean {
    return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  }

  // Traffic-light semantics (same as dns-switch): red → hide to tray
  // (backend maps close-requested to hide), yellow → minimize,
  // green → maximize toggle. No-ops outside Tauri (browser mock).
  const handleClose = () => {
    if (!isTauri()) return;
    getCurrentWindow().close().catch(() => {});
  };
  const handleMinimize = () => {
    if (!isTauri()) return;
    getCurrentWindow().minimize().catch(() => {});
  };
  const handleMaximize = () => {
    if (!isTauri()) return;
    getCurrentWindow().toggleMaximize().catch(() => {});
  };
</script>

<div
  class="title-bar"
  data-testid="title-bar"
  data-tauri-drag-region
  role="toolbar"
  aria-label="Window controls"
>
  <div class="traffic-light-group">
    <button
      class="traffic-dot dot-red"
      onclick={handleClose}
      data-testid="btn-close"
      aria-label="Close"
    >
      <svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true">
        <line x1="3" y1="3" x2="7" y2="7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
        <line x1="7" y1="3" x2="3" y2="7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
      </svg>
    </button>
    <button
      class="traffic-dot dot-yellow"
      onclick={handleMinimize}
      data-testid="btn-minimize"
      aria-label="Minimize"
    >
      <svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true">
        <line x1="2.5" y1="5" x2="7.5" y2="5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
      </svg>
    </button>
    <button
      class="traffic-dot dot-green"
      onclick={handleMaximize}
      data-testid="btn-maximize"
      aria-label="Maximize"
    >
      <svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true">
        <line x1="5" y1="2.5" x2="5" y2="7.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
        <line x1="2.5" y1="5" x2="7.5" y2="5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
      </svg>
    </button>
  </div>
  <div class="title-bar-title">{title}</div>
</div>

<style>
  /* ── Aqua title bar: flat #F4F4F4 → #ECECEC blend, 46px (mockup) ──
     NO border-bottom / separator: the gradient ends exactly at the
     body background color, so the bar blends into the content area
     (deliberate fix carried over from dns-switch).
     Traffic-dot skins live in theme.css (shared with dialogs). */
  .title-bar {
    height: 46px;
    flex-shrink: 0;
    position: relative;
    display: flex;
    align-items: center;
    user-select: none;
    -webkit-app-region: drag;
    background: linear-gradient(180deg, #f4f4f4 0%, #ececec 100%);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.8);
  }

  .traffic-light-group {
    padding-left: 16px;
    height: 100%;
    -webkit-app-region: no-drag;
    z-index: 10;
  }

  /* ── Window title ──────────────────────────────────────────── */
  .title-bar-title {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    font-family: -apple-system, "Lucida Grande", "Segoe UI", sans-serif;
    font-size: 13px;
    font-weight: bold;
    color: var(--text-title);
    letter-spacing: 0.3px;
    text-shadow: 0 1px 0 rgba(255, 255, 255, 0.7);
    pointer-events: none;
    white-space: nowrap;
  }
</style>
