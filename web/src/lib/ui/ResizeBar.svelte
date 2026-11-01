<script lang="ts">
  // A horizontal bar that resizes the element above it by dragging (or the
  // arrow keys); double-click resets. The height lives in the caller, so one
  // value can size the same element in every window.
  let {
    value,
    set,
    min = 24,
    maxFraction = 0.8,
    reset,
    label,
    title,
  }: {
    value: number
    set: (h: number) => void
    min?: number
    /** Largest height, as a fraction of the window height. */
    maxFraction?: number
    reset: number
    label: string
    title: string
  } = $props()

  let drag: { y: number; h: number } | null = null
  function onDown(e: PointerEvent) {
    if (e.button !== 0) return
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
    drag = { y: e.clientY, h: value }
  }
  function onMove(e: PointerEvent) {
    if (!drag) return
    const max = Math.max(min, Math.round(window.innerHeight * maxFraction))
    set(Math.round(Math.max(min, Math.min(max, drag.h + e.clientY - drag.y))))
  }
  function onKey(e: KeyboardEvent) {
    const d = e.key === 'ArrowUp' ? -8 : e.key === 'ArrowDown' ? 8 : 0
    if (!d) return
    e.preventDefault()
    e.stopPropagation()
    set(Math.max(min, value + d))
  }
</script>

<!-- A focusable separator is the ARIA pattern for a resize handle. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  class="divider"
  role="separator"
  aria-orientation="horizontal"
  aria-label={label}
  aria-valuenow={value}
  aria-valuemin={min}
  tabindex="0"
  {title}
  onpointerdown={onDown}
  onpointermove={onMove}
  onpointerup={() => (drag = null)}
  onpointercancel={() => (drag = null)}
  ondblclick={() => set(reset)}
  onkeydown={onKey}
></div>

<style>
  .divider {
    position: relative;
    height: 5px;
    cursor: ns-resize;
    touch-action: none;
    border-top: 1px solid var(--line);
    border-bottom: 1px solid var(--line);
    background: var(--chip);
  }
  .divider:hover,
  .divider:focus-visible {
    background: var(--accent);
    outline: none;
  }
</style>
