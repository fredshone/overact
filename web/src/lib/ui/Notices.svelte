<script lang="ts">
  import { app } from '../state.svelte'
  import Icon from './Icon.svelte'
</script>

<div class="notices" aria-live="polite">
  {#each app.notices as n (n.id)}
    <div class="notice" class:error={n.kind === 'error'} role={n.kind === 'error' ? 'alert' : 'status'}>
      <span>{n.text}</span>
      <button class="ghost icon" aria-label="Dismiss" onclick={() => app.dismiss(n.id)}><Icon name="close" size={12} /></button>
    </div>
  {/each}
</div>

<style>
  .notices {
    position: fixed;
    bottom: 12px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 90;
    display: grid;
    gap: 6px;
    width: max-content;
    max-width: calc(100vw - 32px);
    pointer-events: none;
  }
  .notice {
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-left: 3px solid var(--accent);
    border-radius: 8px;
    padding: 5px 5px 5px 12px;
    box-shadow: var(--shadow);
    pointer-events: auto;
    animation: in 0.15s ease-out;
  }
  .notice.error {
    border-left-color: var(--danger);
  }
  @keyframes in {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }
</style>
