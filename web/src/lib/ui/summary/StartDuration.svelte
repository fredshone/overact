<script lang="ts">
  import { app } from '../../state.svelte'
  import type { Summary } from '../../engine/types'
  import { clock, duration, pct } from '../../format'
  import { mix } from '../../palette'
  import { densityValues, durationTicks, episodeCount, shareTicks, sharedMax, timeTicks, type Tick } from '../../summary'

  let { summary, width }: { summary: Summary; width: number } = $props()
  const GAP = 14
  // Room for the y tick labels (left) and x tick labels (below).
  const LEFT = 30
  const BOTTOM = 15
  const dpr = $derived(app.dpr)
  const view = $derived(app.densityView)
  const acts = $derived(app.activities.filter((a) => a.visible))
  // Plots of 150–240 px, as many per row as fit.
  const cols = $derived(Math.max(1, Math.floor((width + GAP) / (150 + LEFT + GAP))))
  const size = $derived(Math.max(60, Math.min(240, Math.floor((width - GAP * (cols - 1)) / cols) - LEFT)))
  // Every window shades an activity on the same scale.
  const max = $derived(
    sharedMax(
      app.windows.map((w) => w.summary).filter((s): s is Summary => !!s),
      app.activities.length,
      view,
    ),
  )
  const axis0 = $derived(app.header!.time_axis.start)
  const span = $derived(summary.bins * summary.bin)

  // Ticks: x is start time (density, starts) or duration; y is duration
  // (density) or share of episodes (histograms, per activity).
  const xTicks = $derived(view === 'durations' ? durationTicks(span) : timeTicks(axis0, span))
  const yTicks = (code: number): Tick[] => (view === 'density' ? durationTicks(span) : shareTicks(max[code]))
  const xName = $derived(view === 'durations' ? 'duration' : 'start time')
  const yName = $derived(view === 'density' ? 'duration' : 'share of episodes')

  /** Shade from the background through the activity colour to a deeper tone. */
  function shade(color: string, t: number): string {
    const bg = app.colors.canvasBg
    if (t <= 0) return bg
    return t < 0.65 ? mix(bg, color, t / 0.65) : mix(color, app.colors.fg, ((t - 0.65) / 0.35) * 0.45)
  }

  function draw(canvas: HTMLCanvasElement, code: number) {
    $effect(() => {
      const _t = app.redrawToken
      void _t
      const S = Math.round(size * dpr)
      canvas.width = S
      canvas.height = S
      const ctx = canvas.getContext('2d')!
      const bg = app.colors.canvasBg
      ctx.fillStyle = bg
      ctx.fillRect(0, 0, S, S)
      // Faint grid at the ticks.
      ctx.fillStyle = mix(bg, app.colors.fg, 0.07)
      for (const t of xTicks) ctx.fillRect(Math.min(S - 1, Math.round(t.f * S)), 0, 1, S)
      for (const t of yTicks(code)) ctx.fillRect(0, Math.max(0, S - 1 - Math.round(t.f * S)), S, 1)
      const n = summary.bins
      const v = densityValues(summary, code, view)
      const m = max[code] || 1
      const color = app.activities[code].color
      const px = (k: number) => Math.round((k / n) * S)
      if (view === 'density') {
        // Start along x, duration up from the bottom; square-root shading
        // so the long tail stays visible next to the peak.
        for (let i = 0; i < n; i++) {
          for (let j = 0; j < n; j++) {
            const x = v[i * n + j]
            if (!x) continue
            ctx.fillStyle = shade(color, Math.sqrt(x / m))
            ctx.fillRect(px(i), S - px(j + 1), px(i + 1) - px(i), px(j + 1) - px(j))
          }
        }
        // Episodes end by the end of the axis: nothing lies above this diagonal.
        ctx.strokeStyle = mix(bg, app.colors.fg, 0.25)
        ctx.lineWidth = dpr
        ctx.setLineDash([3 * dpr, 3 * dpr])
        ctx.beginPath()
        ctx.moveTo(0, 0)
        ctx.lineTo(S, S)
        ctx.stroke()
      } else {
        ctx.fillStyle = color
        const gap = S / n >= 4 ? 1 : 0
        for (let i = 0; i < n; i++) {
          const h = Math.round((v[i] / m) * S)
          if (h > 0) ctx.fillRect(px(i), S - h, Math.max(1, px(i + 1) - px(i) - gap), h)
        }
      }
    })
  }

  let tip = $state<{ code: number; x: number; y: number; text: string } | null>(null)
  function onMove(e: PointerEvent, code: number) {
    const n = summary.bins
    const b = summary.bin
    const i = Math.max(0, Math.min(n - 1, Math.floor((e.offsetX / size) * n)))
    const j = Math.max(0, Math.min(n - 1, Math.floor(((size - e.offsetY) / size) * n)))
    const v = densityValues(summary, code, view)
    const starts = `starting ${clock(axis0 + i * b)}–${clock(axis0 + (i + 1) * b)}`
    const lasting = (k: number) => `lasting ${duration(k * b)}–${duration((k + 1) * b)}`
    const text =
      view === 'density'
        ? `${starts}, ${lasting(j)}: ${pct(v[i * n + j])}`
        : view === 'starts'
          ? `${starts}: ${pct(v[i])}`
          : `${lasting(i)}: ${pct(v[i])}`
    tip = { code, x: e.offsetX, y: e.offsetY, text }
  }
</script>

<div class="head">
  <div class="switch seg" role="group" aria-label="Start and duration chart">
    <button class:on={view === 'density'} onclick={() => (app.densityView = 'density')} title="Start time along, duration up">Start × duration</button>
    <button class:on={view === 'starts'} onclick={() => (app.densityView = 'starts')} title="Histogram of start times">Starts</button>
    <button class:on={view === 'durations'} onclick={() => (app.densityView = 'durations')} title="Histogram of durations">Durations</button>
  </div>
  <span class="muted small">→ {xName} &nbsp; ↑ {yName}</span>
</div>
<div class="grid" style:grid-template-columns="repeat({cols}, {size + LEFT}px)" style:gap="{GAP}px">
  {#each acts as a (a.code)}
    <figure>
      <figcaption>
        <span class="sw" style:background={a.color}></span>{a.label}
        <span class="muted">{episodeCount(summary, a.code).toLocaleString()} episodes</span>
      </figcaption>
      <div class="frame" style:padding="0 0 {BOTTOM}px {LEFT}px">
        <canvas
          use:draw={a.code}
          style:width="{size}px"
          style:height="{size}px"
          aria-label="{a.label}: {xName} by {yName}"
          onpointermove={(e) => onMove(e, a.code)}
          onpointerleave={() => (tip = null)}
        ></canvas>
        {#each yTicks(a.code) as t (t.label)}
          <span class="yt" style:bottom="{BOTTOM + t.f * size}px" style:right="{size + 4}px">{t.label}</span>
        {/each}
        {#each xTicks as t (t.label)}
          <span class="xt" style:left="{LEFT + t.f * size}px">{t.label}</span>
        {/each}
        {#if tip?.code === a.code}
          <div class="stip" style:left="{LEFT + Math.max(0, Math.min(size - 160, tip.x + 10))}px" style:top="{tip.y + 12}px">
            <div><b>{a.label}</b></div>
            <div class="muted">{tip.text}</div>
          </div>
        {/if}
      </div>
    </figure>
  {/each}
</div>
<p class="caption muted">
  {#if view === 'density'}
    Each activity's episodes by start time (across) and duration (up), in {duration(summary.bin)} bins. Darker means more episodes; the
    scale is square-root and the same for every window. Nothing can lie above the dashed diagonal: those episodes would run past the end of
    the day.
  {:else if view === 'starts'}
    Share of each activity's episodes by start time, in {duration(summary.bin)} bins. Each activity has its own vertical scale, the same in
    every window.
  {:else}
    Share of each activity's episodes by duration, in {duration(summary.bin)} bins. Each activity has its own vertical scale, the same in
    every window.
  {/if}
</p>

<style>
  .head {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px 12px;
    margin-bottom: 8px;
  }
  .switch button {
    font-size: 12px;
  }
  .grid {
    display: grid;
  }
  figure {
    margin: 0;
    min-width: 0;
  }
  figcaption {
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    margin-bottom: 2px;
  }
  figcaption .muted {
    font-size: 11px;
    margin-left: 4px;
  }
  .frame {
    position: relative;
  }
  canvas {
    display: block;
    outline: 1px solid var(--line);
  }
  .yt,
  .xt {
    position: absolute;
    font-size: 10px;
    line-height: 1;
    color: var(--muted);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
    pointer-events: none;
  }
  .yt {
    transform: translateY(50%);
  }
  .xt {
    bottom: 1px;
    transform: translateX(-50%);
  }
</style>
