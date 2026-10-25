<script lang="ts">
  interface Tab {
    id: string;
    label: string;
  }
  interface Props {
    tabs: Tab[];
    active: string;
    onSelect: (id: string) => void;
  }
  let { tabs, active, onSelect }: Props = $props();
</script>

<div class="tab-bar" role="tablist" aria-label="Modules">
  {#each tabs as t (t.id)}
    <button
      class="tab"
      class:active={t.id === active}
      role="tab"
      aria-selected={t.id === active}
      data-testid="tab-{t.id}"
      onclick={() => onSelect(t.id)}
    >
      {t.label}
    </button>
  {/each}
  <span class="tab-hint">click a tab to switch module ←/→</span>
</div>
