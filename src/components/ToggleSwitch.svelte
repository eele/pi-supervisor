<script lang="ts">
  interface Props {
    on: boolean;
    onChange: (on: boolean) => void;
    ariaLabel?: string;
    disabled?: boolean;
    testid?: string;
  }
  let {
    on,
    onChange,
    ariaLabel = "Toggle",
    disabled = false,
    testid,
  }: Props = $props();
</script>

<button
  class="toggle-switch"
  class:on
  role="switch"
  aria-checked={on}
  aria-label={ariaLabel}
  {disabled}
  data-testid={testid}
  onclick={() => {
    if (!disabled) onChange(!on);
  }}
>
  <span class="toggle-label toggle-on-text">ON</span>
  <span class="toggle-label toggle-off-text">OFF</span>
  <span class="toggle-knob"></span>
</button>
