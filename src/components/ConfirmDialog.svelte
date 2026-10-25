<script lang="ts">
  interface Props {
    visible?: boolean;
    title?: string;
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
      <h3>{title}</h3>
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
