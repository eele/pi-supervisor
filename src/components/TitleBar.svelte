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
  /* ── Aqua title bar: flat #F4F4F4 → #ECECEC blend ─────────────
     NO border-bottom / separator: the gradient ends exactly at the
     body background color, so the bar blends into the content area
     (deliberate fix carried over from dns-switch). */
  .title-bar {
    height: 44px;
    flex-shrink: 0;
    position: relative;
    display: flex;
    align-items: center;
    user-select: none;
    -webkit-app-region: drag;
    background: linear-gradient(180deg, #f4f4f4 0%, #ececec 100%);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.8);
  }

  /* ── Traffic light group ───────────────────────────────────── */
  .traffic-light-group {
    display: flex;
    gap: 8px;
    padding-left: 12px;
    height: 100%;
    align-items: center;
    -webkit-app-region: no-drag;
    z-index: 10;
  }

  /* ── Traffic light dot: Aqua "candy sphere" (identical to
        dns-switch: radial base + gloss crescent + ring + shadow) ── */
  .traffic-dot {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 1px solid;
    padding: 0;
    cursor: pointer;
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: filter 0.1s ease;
    flex-shrink: 0;
  }

  .dot-red {
    border-color: #e0443e;
    background: radial-gradient(circle at 50% 42%,
      #ff9a92 0%,
      #ff5f57 45%,
      #d9403a 100%
    );
    box-shadow:
      0 1px 1px rgba(0, 0, 0, 0.18),
      inset 0 -1px 2px rgba(0, 0, 0, 0.12);
  }

  .dot-yellow {
    border-color: #d89e23;
    background: radial-gradient(circle at 50% 42%,
      #ffe082 0%,
      #febc2e 45%,
      #d99424 100%
    );
    box-shadow:
      0 1px 1px rgba(0, 0, 0, 0.18),
      inset 0 -1px 2px rgba(0, 0, 0, 0.12);
  }

  .dot-green {
    border-color: #1aab29;
    background: radial-gradient(circle at 50% 42%,
      #8ad86e 0%,
      #28c840 45%,
      #1e9e22 100%
    );
    box-shadow:
      0 1px 1px rgba(0, 0, 0, 0.18),
      inset 0 -1px 2px rgba(0, 0, 0, 0.12);
  }

  /* ── Signature Aqua gloss: crescent highlight on upper third ── */
  .traffic-dot::before {
    content: "";
    position: absolute;
    top: 1px;
    left: 2px;
    right: 2px;
    height: 45%;
    border-radius: 50% 50% 50% 50% / 80% 80% 20% 20%;
    background: linear-gradient(
      180deg,
      rgba(255, 255, 255, 0.9) 0%,
      rgba(255, 255, 255, 0.4) 60%,
      rgba(255, 255, 255, 0.05) 100%
    );
    pointer-events: none;
  }

  /* ── Glyphs: hidden until group hover (classic macOS behavior) ── */
  .traffic-dot svg {
    position: relative;
    z-index: 2;
    opacity: 0;
    transition: opacity 0.1s ease;
  }
  .traffic-light-group:hover .traffic-dot svg {
    opacity: 1;
  }
  .traffic-dot:hover {
    filter: brightness(1.05);
  }
  .dot-red svg { color: #6b1410; }
  .dot-yellow svg { color: #7a5208; }
  .dot-green svg { color: #1a5c12; }

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
