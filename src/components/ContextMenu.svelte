<script lang="ts">
  interface MenuItem {
    label: string;
    onClick: () => void;
  }
  interface Props {
    visible?: boolean;
    x?: number;
    y?: number;
    items?: MenuItem[];
  }
  let { visible = false, x = 0, y = 0, items = [] }: Props = $props();
</script>

{#if visible}
  <div
    class="context-menu"
    style="left: {x}px; top: {y}px;"
    role="menu"
    data-testid="context-menu"
  >
    {#each items as it, i (i)}
      <div
        class="context-menu-item"
        role="menuitem"
        data-testid="ctx-item-{i}"
        onclick={it.onClick}
      >
        {it.label}
      </div>
    {/each}
  </div>
{/if}
