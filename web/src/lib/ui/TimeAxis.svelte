<script lang="ts">
  import { app, type Win } from '../state.svelte'
  import type { Condition } from '../engine/types'
  import { clock, TICK_STEPS } from '../format'
  import { outside } from './outside'

  let { width, win }: { width: number; win: Win } = $props()

  const ticks = $derived.by(() => {
    const { t0, t1 } = app.time
    const span = t1 - t0
    const step = TICK_STEPS.find((s) => (s / span) * width >= 56) ?? 720
    const a0 = app.header!.time_axis.start
    const out: { x: number; label: string; major: boolean }[] = []
    // Align ticks to absolute clock times.
    const first = Math.ceil((t0 + a0) / step) * step - a0
    for (let m = first; m <= t1; m += step) {
      out.push({ x: ((m - t0) / span) * width, label: clock(a0 + m), major: (a0 + m) % 360 === 0 })
    }
    return out
  })

  const a0 = $derived(app.header!.time_axis.start)
  const minuteAt = (x: number) => {
    const { t0, t1 } = app.time
    return Math.max(0, Math.min(app.axisLen, t0 + (x / Math.max(1, width)) * (t1 - t0)))
  }
  const xOf = (m: number) => {
    const { t0, t1 } = app.time
    return ((m - t0) / (t1 - t0)) * width
  }

  // Drag: brush a time range (then zoom to it or filter on it).
  // Shift-drag: pan. Wheel: zoom around the pointer (shift or sideways: pan).
  // Double-click: whole axis.
  let drag: { x0: number; m0: number; pan: boolean; crop0: number } | null = null
  /** The brushed range waiting for a choice, in minutes. */
  let picked = $state<{ a: number; b: number } | null>(null)
  let activity = $state(0)
  let all = $state(false)

  function onDown(e: PointerEvent) {
    if (e.button !== 0) return
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
    picked = null
    drag = { x0: e.offsetX, m0: minuteAt(e.offsetX), pan: e.shiftKey && app.crop.on, crop0: app.crop.start }
  }

  function onMove(e: PointerEvent) {
    if (!drag) return
    if (drag.pan) {
      const { t0, t1 } = app.time
      const dm = (-(e.offsetX - drag.x0) / Math.max(1, width)) * (t1 - t0)
      app.touch()
      app.panTime(drag.crop0 + dm - app.crop.start)
      return
    }
    if (Math.abs(e.offsetX - drag.x0) < 3) return
    app.brush = { win: win.id, a: drag.m0, b: minuteAt(e.offsetX) }
  }

  function onUp() {
    const b = app.brush
    drag = null
    if (b && b.win === win.id && Math.abs(b.b - b.a) >= 1) {
      const [s, e] = b.a < b.b ? [b.a, b.b] : [b.b, b.a]
      picked = { a: Math.round(s), b: Math.max(Math.round(s) + 1, Math.round(e)) }
    } else {
      app.brush = null
    }
  }

  function close() {
    picked = null
    app.brush = null
  }

  function zoom() {
    if (picked) app.zoomTo(picked.a, picked.b)
    close()
  }

  function filter() {
    if (!picked) return
    const cond: Condition = {
      col: { feature: 'doing_during', activity, start: picked.a, end: picked.b, all },
      negate: false,
      type: 'in',
      values: [1],
    }
    app.setFilter(win, [...($state.snapshot(win.filter) as Condition[]), cond])
    close()
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault()
    const { t0, t1 } = app.time
    const scale = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? width : 1
    const sideways = e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)
    app.touch()
    if (sideways) {
      const d = (e.shiftKey ? e.deltaY || e.deltaX : e.deltaX) * scale
      app.panTime((d / Math.max(1, width)) * (t1 - t0))
    } else {
      app.zoomTime(Math.exp(e.deltaY * scale * 0.002), minuteAt(e.offsetX))
    }
  }

  let el: HTMLDivElement
  $effect(() => {
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => el.removeEventListener('wheel', onWheel)
  })

  const band = $derived(app.brush && app.brush.win === win.id ? [xOf(Math.min(app.brush.a, app.brush.b)), xOf(Math.max(app.brush.a, app.brush.b))] : null)
</script>

<div class="wrap">
  <div
    class="axis"
    class:zoomed={app.crop.on}
    bind:this={el}
    role="slider"
    tabindex="-1"
    aria-label="Time axis"
    aria-valuemin={0}
    aria-valuemax={app.axisLen}
    aria-valuenow={app.time.t0}
    title="Drag: select a time range to zoom to or filter on · Wheel: zoom · Shift-drag or Shift-wheel: pan · Double-click: whole day"
    onpointerdown={onDown}
    onpointermove={onMove}
    onpointerup={onUp}
    onpointercancel={() => {
      drag = null
      app.brush = null
    }}
    ondblclick={() => app.resetTime()}
  >
    {#each ticks as t, i (i)}
      <div class="tick" class:major={t.major} style:left="{t.x}px"><span>{t.label}</span></div>
    {/each}
    {#if band}
      <div class="band" style:left="{band[0]}px" style:width="{Math.max(1, band[1] - band[0])}px"></div>
    {/if}
  </div>
  {#if picked}
    <div class="anchor" style:left="{Math.max(0, Math.min(width - 260, xOf(picked.a)))}px">
      <div class="popover pick" use:outside={close} role="dialog" aria-label="Selected time range">
        <div class="range">{clock(a0 + picked.a)}–{clock(a0 + picked.b)}</div>
        <button class="primary" onclick={zoom}>Zoom to this range</button>
        <div class="muted">Filter this window: doing</div>
        <div class="row">
          <select bind:value={activity} aria-label="Activity">
            {#each app.datasetOf(win).header.activities as a (a.code)}<option value={a.code}>{a.label}</option>{/each}
          </select>
          <select bind:value={all} aria-label="When">
            <option value={false}>at any time</option>
            <option value={true}>throughout</option>
          </select>
          <button onclick={filter}>Add</button>
        </div>
        <button class="ghost cancel" onclick={close}>Cancel</button>
      </div>
    </div>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
    margin-left: 11px;
  }
  .axis {
    position: relative;
    height: 22px;
    border-bottom: 1px solid var(--line);
    overflow: hidden;
    user-select: none;
    touch-action: none;
    font-size: 10px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    cursor: crosshair;
  }
  .axis:hover {
    background: var(--chip);
  }
  .axis.zoomed {
    box-shadow: inset 0 -2px 0 var(--accent);
  }
  .tick {
    position: absolute;
    top: 0;
    bottom: 0;
    border-left: 1px solid var(--line);
    pointer-events: none;
  }
  .tick.major {
    border-left-color: var(--muted);
  }
  .tick span {
    position: absolute;
    left: 3px;
    top: 4px;
    white-space: nowrap;
  }
  .band {
    position: absolute;
    top: 0;
    bottom: 0;
    background: color-mix(in srgb, var(--accent) 30%, transparent);
    border-left: 1px solid var(--accent);
    border-right: 1px solid var(--accent);
    pointer-events: none;
  }
  .anchor {
    margin-top: 2px;
  }
  .pick {
    display: grid;
    gap: 6px;
    width: 260px;
  }
  .range {
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) auto;
    gap: 4px;
  }
  .row select {
    min-width: 0;
  }
  .cancel {
    justify-self: end;
  }
</style>
