<script lang="ts">
  import { app } from '../../state.svelte'
  import type { Summary } from '../../engine/types'
  import { count, pct } from '../../format'
  import { fromLinear, toLinear } from '../../palette'
  import ResizeBar from '../ResizeBar.svelte'

  let { summary, width }: { summary: Summary; width: number } = $props()
  let canvas: HTMLCanvasElement
  const LEFT = 34
  const RIGHT = 44
  const plotW = $derived(Math.max(40, width - LEFT - RIGHT))
  const height = $derived(app.sequenceHeight)
  const dpr = $derived(app.dpr)
  const n = $derived(Math.max(1, summary.n_persons))

  // Each listed sequence is a band whose height is its share of persons (CSS
  // px, top to bottom). Below them, the rarer sequences come from the tail
  // grid. Pixel rows show the mean colour of what falls in them, so thin
  // bands and the tail blend instead of disappearing.
  const layout = $derived.by(() => {
    const bands: { y0: number; y1: number; i: number }[] = []
    let y = 0
    let listed = 0
    summary.sequences.forEach((s, i) => {
      const h = (s.count / n) * height
      bands.push({ y0: y, y1: y + h, i })
      y += h
      listed += s.count
    })
    const rarer = summary.n_persons - listed
    return { bands, tailY: y, rarer, rarerSeqs: summary.n_distinct - summary.sequences.length }
  })
  // Share labels beside bands tall enough to hold one.
  const labels = $derived(layout.bands.filter((b) => b.y1 - b.y0 >= 13).map((b) => ({ y: (b.y0 + b.y1) / 2, text: pct(summary.sequences[b.i].count / n) })))
  // Cumulative share down the left side.
  const yTicks = $derived([0, 0.25, 0.5, 0.75, 1].map((f) => ({ y: f * height, text: `${f * 100}%` })))

  $effect(() => {
    const _t = app.redrawToken
    void _t
    const W = Math.max(1, Math.round(plotW * dpr))
    const H = Math.max(1, Math.round(height * dpr))
    canvas.width = W
    canvas.height = H
    const ctx = canvas.getContext('2d')!
    const bg = toLinear(app.colors.canvasBg)
    const lin = app.activities.map((a) => (a.visible ? toLinear(a.color) : bg))
    // Items in device px, top to bottom: listed sequences, then tail rows.
    type Item = { y0: number; y1: number; rgb: (x: number) => [number, number, number] }
    const items: Item[] = []
    for (const b of layout.bands) {
      const codes = summary.sequences[b.i].codes
      const L = codes.length
      items.push({ y0: b.y0 * dpr, y1: b.y1 * dpr, rgb: (x) => lin[codes[Math.floor(((x + 0.5) * L) / W)]] ?? bg })
    }
    const R = summary.tail_rows
    const C = summary.tail_cols
    const nA = summary.n_activities
    if (layout.rarer > 0 && R && C) {
      const top = layout.tailY * dpr
      const rowH = (H - top) / R
      for (let r = 0; r < R; r++) {
        // Mean colour of each column of this row.
        const cols: [number, number, number][] = []
        for (let c = 0; c < C; c++) {
          let w = 0
          const m: [number, number, number] = [0, 0, 0]
          for (let a = 0; a < nA; a++) {
            const v = summary.tail[(r * C + c) * nA + a]
            if (!v) continue
            const [cr, cg, cb] = lin[a] ?? bg
            m[0] += v * cr
            m[1] += v * cg
            m[2] += v * cb
            w += v
          }
          cols.push(w ? [m[0] / w, m[1] / w, m[2] / w] : bg)
        }
        items.push({ y0: top + r * rowH, y1: top + (r + 1) * rowH, rgb: (x) => cols[Math.floor(((x + 0.5) * C) / W)] })
      }
    }
    const img = ctx.createImageData(W, H)
    const px = img.data
    const acc = new Float32Array(W * 3)
    let first = 0
    for (let y = 0; y < H; y++) {
      acc.fill(0)
      let wsum = 0
      while (first < items.length && items[first].y1 <= y) first++
      for (let k = first; k < items.length && items[k].y0 < y + 1; k++) {
        const it = items[k]
        const w = Math.min(y + 1, it.y1) - Math.max(y, it.y0)
        if (w <= 0) continue
        wsum += w
        for (let x = 0; x < W; x++) {
          const c = it.rgb(x)
          acc[x * 3] += w * c[0]
          acc[x * 3 + 1] += w * c[1]
          acc[x * 3 + 2] += w * c[2]
        }
      }
      // Rounding can leave a sliver of a pixel uncovered: background.
      const rest = Math.max(0, 1 - wsum)
      const norm = 1 / Math.max(1e-9, wsum + rest)
      for (let x = 0, o = y * W * 4; x < W; x++, o += 4) {
        px[o] = fromLinear((acc[x * 3] + rest * bg[0]) * norm)
        px[o + 1] = fromLinear((acc[x * 3 + 1] + rest * bg[1]) * norm)
        px[o + 2] = fromLinear((acc[x * 3 + 2] + rest * bg[2]) * norm)
        px[o + 3] = 255
      }
    }
    ctx.putImageData(img, 0, 0)
    // A hairline under bands tall enough to keep it.
    ctx.fillStyle = app.colors.canvasBg
    for (const b of layout.bands) {
      if ((b.y1 - b.y0) * dpr >= 6) ctx.fillRect(0, Math.round(b.y1 * dpr) - 1, W, 1)
    }
  })

  let tip = $state<{ x: number; y: number; text: string; sub: string } | null>(null)
  function onMove(e: PointerEvent) {
    const y = e.offsetY
    const b = layout.bands.find((b) => y >= b.y0 && y < b.y1)
    if (b) {
      const s = summary.sequences[b.i]
      const k = Math.min(s.codes.length - 1, Math.floor((e.offsetX / plotW) * s.codes.length))
      const thin = (b.y1 - b.y0) * dpr < 1
      tip = {
        x: e.offsetX,
        y,
        text: (thin ? 'About here: ' : '') + s.codes.map((c, i) => (i === k ? `[${app.activities[c]?.label ?? '?'}]` : (app.activities[c]?.label ?? '?'))).join(' → '),
        sub: `${pct(s.count / n)} · ${count(s.count)} ${s.count === 1 ? 'person' : 'persons'} · #${b.i + 1} most common`,
      }
      return
    }
    const R = summary.tail_rows
    const C = summary.tail_cols
    const nA = summary.n_activities
    if (layout.rarer > 0 && y >= layout.tailY && R && C) {
      const r = Math.min(R - 1, Math.floor(((y - layout.tailY) / Math.max(1e-9, height - layout.tailY)) * R))
      const c = Math.min(C - 1, Math.floor((e.offsetX / plotW) * C))
      const cell = summary.tail.slice((r * C + c) * nA, (r * C + c + 1) * nA)
      const sum = cell.reduce((a, v) => a + v, 0) || 1
      const mix = cell
        .map((v, a) => ({ a, v }))
        .filter((x) => x.v)
        .sort((p, q) => q.v - p.v)
        .slice(0, 3)
        .map((x) => `${app.activities[x.a]?.label} ${pct(x.v / sum)}`)
        .join(', ')
      tip = {
        x: e.offsetX,
        y,
        text: `${count(layout.rarerSeqs)} rarer sequences · ${pct(layout.rarer / n)} of persons`,
        sub: `Here (mean colour): ${mix}`,
      }
    } else tip = null
  }
</script>

<div class="chart" style:height="{height}px">
  <div class="yaxis" style:width="{LEFT}px" title="Share of persons, cumulative from the top">
    {#each yTicks as t (t.text)}
      <span class="tick" style:top="{t.y}px">{t.text}</span>
    {/each}
  </div>
  <div class="plot">
    <canvas
      bind:this={canvas}
      style:width="{plotW}px"
      style:height="{height}px"
      aria-label="Activity sequences, most common first"
      onpointermove={onMove}
      onpointerleave={() => (tip = null)}
    ></canvas>
    {#if tip}
      <div class="stip" style:left="{Math.max(0, Math.min(plotW - 220, tip.x + 12))}px" style:top="{tip.y + 12}px">
        <div>{tip.text}</div>
        <div class="muted">{tip.sub}</div>
      </div>
    {/if}
  </div>
  <div class="labels" style:width="{RIGHT}px">
    {#each labels as l (l.y)}
      <span class="tick" style:top="{l.y}px">{l.text}</span>
    {/each}
    {#if layout.rarer > 0 && height - layout.tailY >= 24}
      <span class="tick rarer" style:top="{layout.tailY + 2}px" title="Below this line: the {count(layout.rarerSeqs)} rarer sequences, shown as mean colours">rarer ↓</span>
    {/if}
  </div>
</div>
<ResizeBar
  value={app.sequenceHeight}
  set={(h) => (app.sequenceHeight = h)}
  min={60}
  reset={240}
  label="Resize the sequence charts"
  title="Drag to resize the sequence charts in all windows; double-click to reset"
/>

<style>
  .chart {
    display: flex;
    position: relative;
    margin-bottom: 4px;
  }
  .yaxis,
  .labels {
    position: relative;
    flex: none;
    font-size: 10px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }
  .tick {
    position: absolute;
    transform: translateY(-50%);
    white-space: nowrap;
    line-height: 1;
  }
  .yaxis .tick {
    right: 5px;
  }
  .yaxis .tick:first-child {
    transform: none;
  }
  .yaxis .tick:last-child {
    transform: translateY(-100%);
  }
  .labels .tick {
    left: 5px;
  }
  .labels .rarer {
    transform: none;
    border-top: 1px solid var(--line);
    padding-top: 2px;
  }
  .plot {
    position: relative;
  }
  canvas {
    display: block;
  }
</style>
