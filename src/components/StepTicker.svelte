<script lang="ts">
  import type { RecreateStep } from "../types";

  interface Props {
    steps: RecreateStep[];
  }
  let { steps }: Props = $props();
</script>

<div class="step-ticker" data-testid="step-ticker" aria-label="Recreate progress">
  {#each steps as s, i (i)}
    {#if i > 0}<span class="sep">·</span>{/if}
    {#if s.status === "active"}<span class="mark-active">▸ </span>{/if}<span
      class="step-{s.status}"
      data-testid="ticker-step-{i}"
    >{s.label}</span
    >{#if s.status === "done"}<span class="mark-done"> ✔</span
      >{:else if s.status === "error"}<span class="mark-error"> ✖</span
      >{/if}
  {/each}
</div>
