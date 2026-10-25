<script lang="ts">
  import type { SandboxState, SessionsSnapshot } from "../types";

  interface Props {
    snapshot: SessionsSnapshot;
    sandboxState: SandboxState;
    onAttach: (name: string) => void;
  }
  let { snapshot, sandboxState, onAttach }: Props = $props();
</script>

{#if sandboxState === "not-created"}
  <div class="sessions-empty" data-testid="sessions-empty">Sandbox not created</div>
{:else if !snapshot.has_data}
  <div class="sessions-empty" data-testid="sessions-empty">Loading sessions…</div>
{:else if snapshot.sessions.length === 0}
  <div class="sessions-empty" data-testid="sessions-empty">No sessions yet</div>
{:else}
  <div class="sessions-list" data-testid="sessions-list">
    {#each snapshot.sessions as row (row.name)}
      <div class="session-row" class:dead={!row.live} data-testid="session-row">
        <span class="session-name">{row.name}</span>
        {#if row.live}
          <span class="chip-live" data-testid="chip-live">live</span>
        {:else}
          <span class="session-dead">dead (stale socket)</span>
        {/if}
        <button
          class="btn btn-primary"
          disabled={!row.live}
          data-testid="attach-{row.name}"
          onclick={() => onAttach(row.name)}
        >
          Attach
        </button>
      </div>
    {/each}
  </div>
{/if}
