<script lang="ts">
  import type { RecreateState, SandboxInfo } from "../types";
  import StepTicker from "./StepTicker.svelte";

  interface Props {
    sandbox: SandboxInfo;
    recreate: RecreateState;
    busyRefresh?: boolean;
    busyPush?: boolean;
    onRefresh: () => void;
    onPushTokens: () => void;
    onRecreate: () => void;
  }
  let {
    sandbox,
    recreate,
    busyRefresh = false,
    busyPush = false,
    onRefresh,
    onPushTokens,
    onRecreate,
  }: Props = $props();

  const chipClass = $derived(
    sandbox.state === "detached"
      ? "chip-state detached"
      : sandbox.state === "stopped"
        ? "chip-state stopped"
        : "chip-state not-created"
  );
  const chipText = $derived(
    sandbox.state === "not-created" ? "not created" : sandbox.state
  );
</script>

<div class="sandbox-card">
  <div class="sandbox-head">
    <span class="sandbox-name">pi-ai</span>
    <span class={chipClass} data-testid="sandbox-chip">{chipText}</span>
  </div>

  <div class="sandbox-line">
    <span class="k">kit</span>
    <span class="v">{sandbox.kit || "—"}</span>
  </div>
  <div class="sandbox-line">
    <span class="k">Mount path</span>
    <span class="v">
      {sandbox.mount_path || "—"}{#if sandbox.mount_path}<span class="v-note">
          (virtiofs rw)
        </span>{/if}
    </span>
  </div>
  {#if sandbox.state !== "not-created"}
    <div class="sandbox-line">
      <span class="k">residency</span>
      <span class="v">sbx run -d · permanent, no auto-stop</span>
    </div>
  {/if}

  {#if recreate.active}
    <StepTicker steps={recreate.steps} />
  {:else}
    <div class="sandbox-actions">
      <button class="btn" disabled={busyRefresh} onclick={onRefresh} data-testid="btn-refresh">
        Refresh
      </button>
      <button class="btn" disabled={busyPush} onclick={onPushTokens} data-testid="btn-push-tokens">
        Push Tokens
      </button>
      <span class="spacer"></span>
      <button class="btn btn-primary" onclick={onRecreate} data-testid="btn-recreate">
        Recreate
      </button>
    </div>
  {/if}

  {#if recreate.error}
    <div class="sandbox-error" data-testid="sandbox-error">{recreate.error}</div>
  {/if}
</div>

<style>
  .sandbox-card {
    display: flex;
    flex-direction: column;
  }
  .v-note {
    color: var(--muted);
  }
</style>
