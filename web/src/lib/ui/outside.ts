// Svelte action: calls `handler` on a mousedown outside the element.
// Listening starts on the next tick, so the click that opened the element
// does not immediately close it; a close before then cancels cleanly.

export function outside(node: HTMLElement, handler: () => void) {
  let current = handler
  const onDoc = (e: MouseEvent) => {
    if (!node.contains(e.target as Node)) current()
  }
  const timer = setTimeout(() => document.addEventListener('mousedown', onDoc))
  return {
    update(h: () => void) {
      current = h
    },
    destroy() {
      clearTimeout(timer)
      document.removeEventListener('mousedown', onDoc)
    },
  }
}
