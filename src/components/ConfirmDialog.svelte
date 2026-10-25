<script lang="ts">
  interface Props {
    visible?: boolean;
    title?: string;
    /** Bold question line under the header (mockup: Recreate "pi-ai" …?) */
    question?: string;
    message?: string;
    confirmLabel?: string;
    cancelLabel?: string;
    danger?: boolean;
    onConfirm: () => void;
    onCancel: () => void;
  }
  let {
    visible = false,
    title = "",
    question = "",
    message = "",
    confirmLabel = "OK",
    cancelLabel = "Cancel",
    danger = true,
    onConfirm,
    onCancel,
  }: Props = $props();
</script>

{#if visible}
  <div class="confirm-overlay" data-testid="confirm-overlay">
    <div class="confirm-dialog" role="alertdialog" aria-label={title}>
      <div class="confirm-head">
        <div class="traffic-light-group" aria-hidden="true">
          <span class="traffic-dot dot-red"></span>
          <span class="traffic-dot dot-yellow"></span>
          <span class="traffic-dot dot-green"></span>
        </div>
        <span class="confirm-title">{title}</span>
      </div>
      {#if question}<div class="confirm-question">{question}</div>{/if}
      <p>{message}</p>
      <div class="btn-group">
        <button class="btn" data-testid="btn-confirm-no" onclick={onCancel}>
          {cancelLabel}
        </button>
        <button
          class="btn {danger ? 'btn-danger-solid' : 'btn-primary'}"
          data-testid="btn-confirm-yes"
          onclick={onConfirm}
        >
          {confirmLabel}
        </button>
      </div>
    </div>
  </div>
{/if}
