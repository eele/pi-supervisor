<script lang="ts">
  import type { ServiceId, ServiceInfo } from "../types";

  interface Props {
    info: ServiceInfo;
    onToggle: (id: ServiceId) => void;
  }
  let { info, onToggle }: Props = $props();
</script>

<div class="service-row" data-testid="service-row-{info.id}">
  <span class="state-dot {info.state}" aria-label={info.state}></span>
  <div class="service-meta">
    <div class="service-name">{info.name}</div>
    <div class="service-subtitle">{info.subtitle}</div>
  </div>
  {#if info.state === "running"}
    <button class="btn" data-testid="btn-toggle-{info.id}" onclick={() => onToggle(info.id)}>
      Stop
    </button>
  {:else}
    <button class="btn" data-testid="btn-toggle-{info.id}" onclick={() => onToggle(info.id)}>
      Start
    </button>
  {/if}
</div>
