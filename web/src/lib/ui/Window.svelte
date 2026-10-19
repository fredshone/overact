<script lang="ts">
  import { onMount, untrack } from 'svelte'
  import { app, type Win } from '../state.svelte'
  import type { Hover } from '../engine/client'
  import type { ColRef } from '../engine/types'
  import { clock, colName, formatValue, colMeta } from '../format'
  import { ramp } from '../palette'
  import WindowHeader from './WindowHeader.svelte'
  import ProfileStrip from './ProfileStrip.svelte'
  import TimeAxis from './TimeAxis.svelte'

  let { win }: { win: Win } = $props()

  let body: HTMLDivElement
  let canvasEl: HTMLCanvasElement
  let overlay: HTMLCanvasElement
  let gutterEl: HTMLCanvasElement
  let cssW = $state(0)
  let cssH = $state(0)
  let attached = $state(false)
  let pointer = $state<{ x: number; y: number } | null>(null)
  let hover = $state<Hover | null>(null)
  let pinned = $state<Hover | null>(null)
  let gutterVals = $state.raw<Float64Array | null>(null)
  let gutterHover = $state<{ y: number; text: string } | null>(null)

  const dpr = $derived(app.dpr)
  // A window's dataset never changes (showing another replaces the window).
  const ds = untrack(() => app.datasetOf(win))
  const client = ds.client

  const rows = $derived(app.rowRange(win))
  // The shared sort, as this dataset sees it.
  const sortKeys = $derived(app.sortFor(ds))
  // The gutter shows the primary key, if this dataset has it.
  const gutterKey = $derived(app.sort.length && !app.sortMissing(0).includes(ds.name) ? sortKeys[0] : undefined)
  // Scroll thumb when only some rows are shown.
  const thumb = $derived.by(() => {
    const { span, total } = app.extent(win)
    if (!(total > span) || !cssH) return null
    const [rs] = rows
    const height = Math.max(12, (span / total) * cssH)
    return { top: Math.min(cssH - height, (rs / total) * cssH), height }
  })
  const usedCols = $derived<ColRef[]>([...win.filter.map((c) => c.col), ...sortKeys.map((k) => k.col)])

  onMount(() => {
    const off = canvasEl.transferControlToOffscreen()
    client.attachCanvas(win.id, off).then(() => {
      attached = true
      app.submit(win)
    })
    const ro = new ResizeObserver(() => {
      cssW = body.clientWidth
      cssH = body.clientHeight
      app.windowHeights[win.id] = cssH
    })
    ro.observe(body)
    overlay.addEventListener('wheel', onWheel, { passive: false })
    return () => {
      ro.disconnect()
      overlay.removeEventListener('wheel', onWheel)
      clearTimeout(exactTimer)
      clearTimeout(busyTimer)
      delete app.windowHeights[win.id]
    }
  })

  // --- Drawing (the worker owns the main canvas) ---
  let exactTimer: ReturnType<typeof setTimeout> | undefined
  $effect(() => {
    const _deps = [win.version, app.redrawToken, app.renderMode, app.maxSamples, app.exactPass, app.interacting]
    void _deps
    if (!attached || !win.version || cssW < 1 || cssH < 1) return
    const { t0, t1 } = app.time
    const [rs, re] = rows
    const width = Math.round(cssW * dpr)
    // Single-threaded, the per-pixel colour pass is about half the raster
    // cost: while moving, draw at half the horizontal resolution (the canvas
    // is stretched back to full width).
    const coarse = app.interacting && app.threads === 1 && width > 400
    const params = {
      width: coarse ? Math.round(width / 2) : width,
      height: Math.round(cssH * dpr),
      rowStart: rs,
      rowEnd: re,
      t0,
      t1,
      // Fewer samples while moving; full quality once the view settles.
      maxSamples: app.interacting ? Math.min(8, app.maxSamples) : app.maxSamples,
      mode: app.renderMode,
    }
    client.draw(win.id, params)
    clearTimeout(exactTimer)
    const sampled = app.renderMode !== 2 && (re - rs) / params.height > app.maxSamples
    if (app.exactPass && sampled && !app.interacting) {
      exactTimer = setTimeout(() => client.draw(win.id, params, true), 600)
    }
  })

  // --- Busy: dim the stale rows when a job takes more than a moment ---
  let busyTimer: ReturnType<typeof setTimeout> | undefined
  let showBusy = $state(false)
  $effect(() => {
    clearTimeout(busyTimer)
    if (!win.busy) {
      showBusy = false
      return
    }
    busyTimer = setTimeout(() => (showBusy = true), 300)
  })

  // --- Sort-key gutter ---
  // Replies can arrive out of order: only the latest request's is used.
  let gutterReq = 0
  $effect(() => {
    const key = gutterKey
    const _v = win.version
    void _v
    const [rs, re] = rows
    const h = Math.max(1, Math.round(cssH * dpr))
    const req = ++gutterReq
    if (!key || !win.version) {
      gutterVals = null
      return
    }
    client.gutter(win.id, $state.snapshot(key.col) as ColRef, h, rs, re).then(
      (v) => {
        if (req === gutterReq) gutterVals = v
      },
      () => {
        if (req === gutterReq) gutterVals = null
      },
    )
  })

  $effect(() => {
    const vals = gutterVals
    const g = gutterEl
    const range = app.sortRange
    if (!g) return
    const h = Math.max(1, Math.round(cssH * dpr))
    g.width = 1
    g.height = h
    const ctx = g.getContext('2d')!
    ctx.clearRect(0, 0, 1, h)
    if (!vals || !range) return
    const { min, max } = range
    const span = max - min || 1
    const desc = gutterKey?.desc
    // Runs of equal values (the usual case for sorted keys) are one rectangle.
    for (let y = 0; y < vals.length; ) {
      const v = vals[y]
      let end = y + 1
      while (end < vals.length && vals[end] === v) end++
      if (!Number.isNaN(v)) {
        const t = (v - min) / span
        ctx.fillStyle = ramp(desc ? 1 - t : t)
        ctx.fillRect(0, y, 1, end - y)
      }
      y = end
    }
  })

  // --- Overlay: hover line and highlighted row ---
  $effect(() => {
    const o = overlay
    if (!o) return
    const w = Math.round(cssW * dpr)
    const h = Math.round(cssH * dpr)
    if (o.width !== w || o.height !== h) {
      o.width = w
      o.height = h
    }
    const ctx = o.getContext('2d')!
    ctx.clearRect(0, 0, w, h)
    const { t0, t1 } = app.time
    const m = app.hover.minute
    const { fg, accent } = app.colors
    if (m !== null) {
      const x = ((m - t0) / (t1 - t0)) * w
      ctx.fillStyle = fg
      ctx.globalAlpha = 0.85
      ctx.fillRect(Math.round(x), 0, Math.max(1, Math.round(dpr)), h)
      ctx.globalAlpha = 1
    }
    const b = app.brush
    if (b && b.win === win.id) {
      const xa = ((Math.min(b.a, b.b) - t0) / (t1 - t0)) * w
      const xb = ((Math.max(b.a, b.b) - t0) / (t1 - t0)) * w
      ctx.fillStyle = accent
      ctx.globalAlpha = 0.18
      ctx.fillRect(xa, 0, Math.max(1, xb - xa), h)
      ctx.globalAlpha = 1
    }
    const [rs, re] = rows
    const lw = Math.max(1, Math.round(dpr))
    const mark = (pos: number, color: string) => {
      const y0 = ((pos - rs) / (re - rs)) * h
      const y1 = ((pos + 1 - rs) / (re - rs)) * h
      if (y1 - y0 >= 4 * lw) {
        ctx.strokeStyle = color
        ctx.lineWidth = lw
        ctx.strokeRect(0.5, Math.round(y0) + 0.5, w - 1, Math.round(y1 - y0) - 1)
      } else {
        // Thin rows: lines just above and below the pixel row, leaving it visible.
        const a = Math.floor(y0)
        const b = Math.max(a + 1, Math.ceil(y1))
        ctx.fillStyle = color
        ctx.fillRect(0, a - lw, w, lw)
        ctx.fillRect(0, b, w, lw)
      }
    }
    if (pointer && hover) mark(rowAt(pointer.y), fg)
    if (pinnedPos !== null) mark(pinnedPos, accent)
  })

  let pinnedPos = $state<number | null>(null)

  // --- Interaction: scrolling moves through the rows when not all are shown ---
  function minuteAt(x: number): number {
    const { t0, t1 } = app.time
    return t0 + (x / cssW) * (t1 - t0)
  }

  function onWheel(e: WheelEvent) {
    if (e.ctrlKey || e.metaKey) return // leave browser zoom alone
    const { span, total } = app.extent(win)
    if (!(total > span)) return
    e.preventDefault()
    const scale = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? cssH : 1
    app.scrollRows(e.deltaY * scale, cssH, win)
  }

  /**
   * Row position under CSS y. Zoomed out (several rows per device pixel), it
   * snaps to the first person of that pixel row.
   */
  function rowAt(y: number): number {
    const [rs, re] = rows
    const h = Math.round(cssH * dpr)
    const step = (re - rs) / Math.max(1, h)
    const pos = step >= 1 ? rs + Math.floor(y * dpr) * step : rs + (y / cssH) * (re - rs)
    return Math.max(0, Math.floor(pos))
  }

  // One hover request in flight at a time: while the worker is busy with a
  // job step, pointer moves only update the position asked for next.
  let hoverBusy = false
  let hoverWant: number | null = null
  let hoverReq = 0
  function requestHover(pos: number) {
    hoverWant = pos
    if (hoverBusy) return
    hoverBusy = true
    hoverWant = null
    const req = ++hoverReq
    client
      .hover(win.id, pos, $state.snapshot(usedCols) as ColRef[])
      .then(
        (h) => {
          if (req === hoverReq) hover = h
        },
        () => {},
      )
      .finally(() => {
        hoverBusy = false
        if (hoverWant !== null) requestHover(hoverWant)
      })
  }

  function onPointerMove(e: PointerEvent) {
    const r = overlay.getBoundingClientRect()
    const x = e.clientX - r.left
    const y = e.clientY - r.top
    pointer = { x, y }
    app.hover = { minute: minuteAt(x), win: win.id }
    const pos = rowAt(y)
    if (pos >= rows[1]) {
      hoverWant = null
      ++hoverReq
      hover = null
      return
    }
    requestHover(pos)
  }

  function onPointerUp(e: PointerEvent) {
    if (e.button !== 0) return
    const r = overlay.getBoundingClientRect()
    const pos = rowAt(e.clientY - r.top)
    if (pos >= rows[1]) return
    if (pinnedPos === pos) {
      pinnedPos = null
      pinned = null
      return
    }
    client.hover(win.id, pos, $state.snapshot(usedCols) as ColRef[]).then((h) => {
      pinned = h
      pinnedPos = h ? pos : null
    })
  }

  function onLeave() {
    pointer = null
    hoverWant = null
    ++hoverReq
    hover = null
    if (app.hover.win === win.id) app.hover = { minute: null, win: null }
  }

  // Unpin when the rows change underneath.
  $effect(() => {
    const _v = win.version
    void _v
    pinned = null
    pinnedPos = null
  })

  // --- Readouts ---
  function activityAt(h: Hover, m: number): string {
    const s = h.person.segments.find(([a, b]) => a <= m && m < b)
    return s ? (app.activities[ds.toUnion[s[2]]]?.label ?? '?') : ''
  }

  function onGutterMove(e: PointerEvent) {
    const y = e.offsetY
    const v = gutterVals?.[Math.floor(y * dpr)]
    const key = gutterKey
    if (!key || v === undefined) return
    gutterHover = { y, text: `${colName(H, key.col)}: ${formatValue(H, key.col, Number.isNaN(v) ? null : v)}` }
  }

  const H = ds.header

  // Tooltip beside the pointer, flipped to the other side near the edges.
  const TIP_W = 220
  const tipLeft = $derived(pointer && pointer.x + 14 + TIP_W > cssW ? Math.max(0, pointer.x - 14 - TIP_W) : (pointer?.x ?? 0) + 14)
  let tipEl = $state<HTMLDivElement>()
  const tipTop = $derived.by(() => {
    if (!pointer) return 0
    const th = tipEl?.offsetHeight ?? 100
    return pointer.y + 14 + th > cssH ? Math.max(0, pointer.y - 14 - th) : pointer.y + 14
  })
</script>

<section class="window" class:dragging={app.draggingWin === win.id} data-win={win.id} aria-label={win.title}>
  <WindowHeader {win} />
  {#if app.showProfiles}
    <ProfileStrip {win} width={cssW} />
  {/if}
  <TimeAxis width={cssW} {win} />
  <div class="main">
    <div class="gutter" title={gutterKey ? `Sorted by ${colName(H, gutterKey.col)}` : app.sort.length ? `Not sorted by the first key: not in ${ds.name}` : 'Not sorted'}>
      <canvas
        bind:this={gutterEl}
        onpointermove={onGutterMove}
        onpointerleave={() => (gutterHover = null)}
      ></canvas>
      {#if gutterHover}
        <div class="gtip" style:top="{gutterHover.y}px">{gutterHover.text}</div>
      {/if}
    </div>
    <div class="body" bind:this={body}>
      <canvas class="raster" bind:this={canvasEl} style:width="{cssW}px" style:height="{cssH}px"></canvas>
      <canvas
        class="overlay"
        bind:this={overlay}
        style:width="{cssW}px"
        style:height="{cssH}px"
        onpointermove={onPointerMove}
        onpointerup={onPointerUp}
        onpointerleave={onLeave}
      ></canvas>
      {#if thumb}
        <div class="thumb" style:top="{thumb.top}px" style:height="{thumb.height}px"></div>
      {/if}
      {#if win.count === 0}
        <div class="empty muted">No persons match this filter.</div>
      {/if}
      {#if showBusy}
        <div class="busy" role="status">
          <span>{win.count === null ? 'Loading' : 'Updating'}… {Math.round(win.progress * 100)}%</span>
        </div>
      {/if}
      {#if hover && pointer && app.hover.minute !== null && hover.person.index !== pinned?.person.index}
        <div class="tip" bind:this={tipEl} style:left="{tipLeft}px" style:top="{tipTop}px" style:width="{TIP_W}px">
          <div><b>{hover.person.id ?? `#${hover.person.index}`}</b> · {activityAt(hover, Math.floor(app.hover.minute))}</div>
          {#each usedCols as c, i (i)}
            <div class="muted">{colName(H, c)}: {formatValue(H, c, hover.values[i])}</div>
          {/each}
          <div class="muted small">click to pin</div>
        </div>
      {/if}
    </div>
  </div>
  {#if pinned}
    <div class="pinned">
      <div class="pin-head">
        <b>{pinned.person.id ?? `#${pinned.person.index}`}</b>
        <button class="ghost" aria-label="Unpin" onclick={() => { pinned = null; pinnedPos = null }}>×</button>
      </div>
      {#each usedCols as c, i (i)}
        <div class="muted">{colName(H, c)}: {formatValue(H, c, pinned.values[i])}</div>
      {/each}
      <ol class="segs">
        {#each pinned.person.segments as [s, e, code], i (i)}
          <li>
            <span class="sw" style:background={app.activities[ds.toUnion[code]]?.color}></span>
            {clock(H.time_axis.start + s)}–{clock(H.time_axis.start + e)} {app.activities[ds.toUnion[code]]?.label}
          </li>
        {/each}
      </ol>
    </div>
  {/if}
</section>


<style>
  .window {
    position: relative;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 8px;
    overflow: hidden;
  }
  .window.dragging {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent), var(--shadow);
  }
  .main {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .gutter {
    position: relative;
    width: 10px;
    flex: none;
    border-right: 1px solid var(--line);
  }
  .gutter canvas {
    width: 100%;
    height: 100%;
    display: block;
    image-rendering: pixelated;
  }
  .gtip {
    position: absolute;
    left: 14px;
    z-index: 5;
    transform: translateY(-50%);
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 1px 6px;
    white-space: nowrap;
    pointer-events: none;
    box-shadow: var(--shadow);
  }
  .body {
    position: relative;
    flex: 1;
    min-width: 0;
    background: var(--canvas-bg);
    overflow: hidden;
  }
  .raster,
  .overlay {
    position: absolute;
    inset: 0;
    display: block;
  }
  .thumb {
    position: absolute;
    right: 2px;
    width: 4px;
    border-radius: 2px;
    background: var(--fg);
    opacity: 0.35;
    pointer-events: none;
  }
  .overlay {
    cursor: pointer;
  }
  .empty {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
  }
  .tip {
    position: absolute;
    pointer-events: none;
    background: color-mix(in srgb, var(--panel) 92%, transparent);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 4px 8px;
    font-size: 12px;
    box-shadow: var(--shadow);
  }
  .busy {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--canvas-bg) 45%, transparent);
    pointer-events: none;
    animation: fade 0.2s ease-out;
  }
  .busy span {
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 3px 12px;
    font-size: 12px;
    box-shadow: var(--shadow);
    font-variant-numeric: tabular-nums;
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  .pinned {
    position: absolute;
    right: 8px;
    bottom: 8px;
    width: 240px;
    max-height: 60%;
    overflow: auto;
    background: var(--panel);
    border: 1px solid var(--accent);
    border-radius: 8px;
    padding: 8px 10px;
    box-shadow: var(--shadow);
    z-index: 10;
    font-size: 12px;
  }
  .pin-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .segs {
    margin: 6px 0 0;
    padding-left: 0;
    list-style: none;
    font-family: var(--mono);
    font-size: 11px;
  }
</style>
