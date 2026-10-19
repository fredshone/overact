<script lang="ts">
  import { app, type Win } from '../state.svelte'
  import { clock, pct } from '../format'
  import { hexToInt } from '../palette'

  let { win, width }: { win: Win; width: number } = $props()
  let canvas: HTMLCanvasElement
  const dpr = $derived(app.dpr)
  const MIN_HEIGHT = 24
  const DEFAULT_HEIGHT = 108

  // Dragging the divider below the strip resizes the strips of all windows.
  let resize: { y: number; h: number } | null = null
  function onDown(e: PointerEvent) {
    if (e.button !== 0) return
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
    resize = { y: e.clientY, h: app.profileHeight }
  }
  function onMove(e: PointerEvent) {
    if (!resize) return
    const max = Math.max(MIN_HEIGHT, Math.round(window.innerHeight * 0.6))
    app.profileHeight = Math.round(Math.max(MIN_HEIGHT, Math.min(max, resize.h + e.clientY - resize.y)))
  }
  function onKey(e: KeyboardEvent) {
    const d = e.key === 'ArrowUp' ? -8 : e.key === 'ArrowDown' ? 8 : 0
    if (!d) return
    e.preventDefault()
    e.stopPropagation()
    app.profileHeight = Math.max(MIN_HEIGHT, app.profileHeight + d)
  }

  // Hover: the shared time line, and this window's activity mix at that minute.
  const lineX = $derived.by(() => {
    const m = app.hover.minute
    const { t0, t1 } = app.time
    return m === null ? null : ((m - t0) / (t1 - t0)) * width
  })
  const mix = $derived.by(() => {
    const m = app.hover.minute
    const p = win.profile
    const n = win.count ?? 0
    const L = app.axisLen
    if (m === null || !p || !n || m < 0 || m >= L) return null
    const t = Math.floor(m)
    return app.activities
      .map((a) => ({ a, share: p[a.code * L + t] / n }))
      .filter((x) => x.share > 0)
      .sort((x, y) => y.share - x.share)
      .slice(0, 4)
  })

  function onStripMove(e: PointerEvent) {
    const { t0, t1 } = app.time
    app.hover = { minute: t0 + (e.offsetX / Math.max(1, width)) * (t1 - t0), win: win.id }
  }
  function onStripLeave() {
    if (app.hover.win === win.id) app.hover = { minute: null, win: null }
  }

  /** `#rrggbb` as a little-endian RGBA pixel (as `Uint32Array` views of ImageData see it). */
  function pixel(hex: string): number {
    const c = hexToInt(hex)
    return (0xff000000 | ((c & 0xff) << 16) | (c & 0xff00) | ((c >> 16) & 0xff)) >>> 0
  }

  // Exact share of each activity by minute, stacked from the top in activity
  // order (so the first, usually most common, activities are at the top).
  // Drawn into one pixel buffer: a column per device pixel, computed once
  // per minute.
  $effect(() => {
    const p = win.profile
    const n = win.count ?? 0
    const L = app.axisLen
    const { t0, t1 } = app.time
    const acts = app.activities
    const _t = app.redrawToken
    void _t
    const W = Math.max(1, Math.round(width * dpr))
    const H = Math.max(1, Math.round(app.profileHeight * dpr))
    canvas.width = W
    canvas.height = H
    const ctx = canvas.getContext('2d')!
    if (!p || !n) {
      ctx.clearRect(0, 0, W, H)
      return
    }
    const bg = pixel(app.colors.canvasBg)
    const colors = acts.map((a) => (a.visible ? pixel(a.color) : bg))
    const img = ctx.createImageData(W, H)
    const px = new Uint32Array(img.data.buffer)
    const col = new Uint32Array(H)
    let lastM = -1
    for (let x = 0; x < W; x++) {
      const m = Math.floor(t0 + ((x + 0.5) / W) * (t1 - t0))
      if (m < 0 || m >= L) continue
      if (m !== lastM) {
        col.fill(bg)
        let y = 0
        acts.forEach((a, i) => {
          const share = p[a.code * L + m] / n
          if (!share) return
          const y1 = y + share * H
          col.fill(colors[i], Math.round(y), Math.round(y1))
          y = y1
        })
        lastM = m
      }
      for (let y = 0, k = x; y < H; y++, k += W) px[k] = col[y]
    }
    ctx.putImageData(img, 0, 0)
  })
</script>

<div class="wrap" class:stale={win.profilePending} role="presentation" onpointermove={onStripMove} onpointerleave={onStripLeave}>
  <canvas class="strip" bind:this={canvas} style:height="{app.profileHeight}px" aria-label="Activity shares by time of day"></canvas>
  {#if win.profilePending}
    <div class="pending muted" title="The shares above are for the previous rows; new ones follow">updating…</div>
  {/if}
  {#if lineX !== null && lineX >= 0 && lineX <= width}
    <div class="line" style:left="{lineX}px"></div>
  {/if}
  {#if mix && lineX !== null && app.hover.minute !== null}
    <div class="mix" style:left="{Math.min(width - 170, Math.max(4, lineX + 8))}px">
      <div class="mix-title">{clock(app.header!.time_axis.start + app.hover.minute)}</div>
      {#each mix as { a, share } (a.code)}
        <div><span class="sw" style:background={a.color}></span>{a.label} <b>{pct(share)}</b></div>
      {/each}
    </div>
  {/if}
</div>
<!-- A focusable separator is the ARIA pattern for a resize handle. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  class="divider"
  role="separator"
  aria-orientation="horizontal"
  aria-label="Resize activity share strips"
  aria-valuenow={app.profileHeight}
  aria-valuemin={MIN_HEIGHT}
  tabindex="0"
  title="Drag to resize the strips in all windows; double-click to reset"
  onpointerdown={onDown}
  onpointermove={onMove}
  onpointerup={() => (resize = null)}
  onpointercancel={() => (resize = null)}
  ondblclick={() => (app.profileHeight = DEFAULT_HEIGHT)}
  onkeydown={onKey}
></div>

<style>
  .wrap {
    position: relative;
    margin-left: 11px;
  }
  .strip {
    display: block;
    width: 100%;
    transition: opacity 0.2s;
  }
  .stale .strip {
    opacity: 0.45;
  }
  .pending {
    position: absolute;
    right: 6px;
    top: 4px;
    font-size: 11px;
    background: var(--panel);
    border-radius: 999px;
    padding: 0 6px;
    pointer-events: none;
  }
  .line {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1px;
    background: var(--fg);
    opacity: 0.85;
    pointer-events: none;
  }
  .mix {
    position: absolute;
    top: 4px;
    z-index: 6;
    width: 160px;
    pointer-events: none;
    background: color-mix(in srgb, var(--panel) 92%, transparent);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 4px 8px;
    font-size: 12px;
    box-shadow: var(--shadow);
  }
  .mix-title {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--muted);
  }
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
