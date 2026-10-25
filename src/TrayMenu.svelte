<script lang="ts">
  // Tray right-click menu (mockup 3): #F6F6F6 rounded card, #9D9D9D
  // border, hovered item = solid Aqua blue (#3876D6) with white text,
  // Open | ─── | Exit. Lives in its own frameless transparent window
  // (backend positions it at the tray icon and hides it on blur).
  import { invoke } from "@tauri-apps/api/core";

  let hover = $state<"open" | "exit" | null>(null);

  function act(action: "open" | "exit") {
    invoke("tray_menu_action", { action }).catch(() => {
      /* outside Tauri (mock): no-op */
    });
  }
</script>

<svelte:window on:contextmenu={(e) => e.preventDefault()} />

<div class="tray-menu-root">
  <div class="tray-menu" data-testid="tray-menu">
    <button
      class="tray-item"
      class:hovered={hover === "open"}
      data-testid="tray-item-open"
      aria-label="Open"
      onmouseenter={() => (hover = "open")}
      onmouseleave={() => (hover = null)}
      onclick={() => act("open")}
    >
      Open
    </button>
    <div class="tray-sep" role="separator"></div>
    <button
      class="tray-item"
      class:hovered={hover === "exit"}
      data-testid="tray-item-exit"
      aria-label="Exit"
      onmouseenter={() => (hover = "exit")}
      onmouseleave={() => (hover = null)}
      onclick={() => act("exit")}
    >
      Exit
    </button>
  </div>
</div>

<style>
  /* 10px transparent margin carries the drop shadow inside the
     frameless transparent window (same trick as the main window). */
  .tray-menu-root {
    position: absolute;
    inset: 0;
    padding: 10px;
    background: transparent;
    font-family: var(--font-main);
    user-select: none;
  }

  .tray-menu {
    width: 150px;
    background: #f6f6f6;
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 4px 10px rgba(0, 0, 0, 0.35), 0 1px 3px rgba(0, 0, 0, 0.2);
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .tray-item {
    height: 26px;
    padding: 0 10px;
    border: none;
    border-radius: 4px;
    background: transparent;
    text-align: left;
    font-family: var(--font-main);
    font-size: 12px;
    color: var(--text-primary);
    cursor: default;
  }

  .tray-item.hovered {
    background: var(--accent);
    color: #fff;
    text-shadow: 0 -1px 0 rgba(0, 0, 0, 0.25);
  }

  .tray-sep {
    height: 1px;
    margin: 2px 0;
    background: var(--border-light);
  }
</style>
