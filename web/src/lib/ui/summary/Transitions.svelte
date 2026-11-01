<script lang="ts">
  import { app } from '../../state.svelte'
  import type { Counted, Summary } from '../../engine/types'
  import { pct } from '../../format'
  import { hexToInt, mix } from '../../palette'
  import { transitionShares } from '../../summary'

  let { summary, width }: { summary: Summary; width: number } = $props()
  const N_GRAMS = 8
  const LABEL_W = 92
  const HEAD_H = 66
  const dpr = $derived(app.dpr)
  const nA = $derived(summary.n_activities)
  // Visible activities, then the start (rows) / end (columns) of the day.
  const states = $derived([...app.activities.filter((a) => a.visible).map((a) => a.code), nA])
  const label = (s: number, from: boolean) => (s === nA ? (from ? 'day start' : 'day end') : app.activities[s].label)
  const colorOf = (s: number) => (s === nA ? mix(app.colors.canvasBg, app.colors.fg, 0.55) : app.activities[s].color)
  const cell = $derived(Math.max(10, Math.min(40, Math.floor((width - LABEL_W) / states.length))))
  const shares = $derived(transitionShares(summary))
  // Shares printed in cells when they fit.
  const showText = $derived(cell >= 26)

  /** Text colour readable on a fill. */
  function ink(hex: string): string {
    const c = hexToInt(hex)
    const lum = 0.2126 * ((c >> 16) & 255) + 0.7152 * ((c >> 8) & 255) + 0.0722 * (c & 255)
    return lum > 140 ? '#1d1d1f' : '#ffffff'
  }

  /** A cell's fill: the destination's legend colour, stronger for larger shares. */
  const fill = (v: number, to: number) => (v ? mix(app.colors.canvasBg, colorOf(to), 0.12 + 0.88 * Math.sqrt(v)) : mix(app.colors.canvasBg, app.colors.fg, 0.04))

  let canvas: HTMLCanvasElement
  $effect(() => {
    const _t = app.redrawToken
    void _t
    const n = states.length
    const S = Math.round(cell * dpr)
    canvas.width = n * S
    canvas.height = n * S
    const ctx = canvas.getContext('2d')!
    ctx.fillStyle = app.colors.canvasBg
    ctx.fillRect(0, 0, n * S, n * S)
    const gap = S >= 12 ? Math.round(dpr) : 0
    ctx.font = `${10 * dpr}px system-ui, sans-serif`
    ctx.textAlign = 'center'
    ctx.textBaseline = 'middle'
    states.forEach((a, i) =>
      states.forEach((b, j) => {
        const v = shares[a * (nA + 1) + b]
        const f = fill(v, b)
        ctx.fillStyle = f
        ctx.fillRect(j * S, i * S, S - gap, S - gap)
        if (showText && v >= 0.005) {
          ctx.fillStyle = ink(f)
          ctx.fillText(v >= 0.995 ? '100' : (v * 100).toFixed(v < 0.1 ? 1 : 0), j * S + (S - gap) / 2, i * S + (S - gap) / 2)
        }
      }),
    )
  })

  let tip = $state<{ x: number; y: number; text: string; sub: string } | null>(null)
  function onMove(e: PointerEvent) {
    const i = Math.floor(e.offsetY / cell)
    const j = Math.floor(e.offsetX / cell)
    if (i < 0 || j < 0 || i >= states.length || j >= states.length) return (tip = null)
    const [a, b] = [states[i], states[j]]
    const c = summary.trans[a * (nA + 1) + b]
    const v = shares[a * (nA + 1) + b]
    tip = {
      x: e.offsetX,
      y: e.offsetY,
      text: `${label(a, true)} → ${label(b, false)}`,
      sub:
        a === nA
          ? b === nA
            ? ''
            : `${pct(v)} of persons start the day with ${label(b, false)}`
          : b === nA
            ? `${pct(v)} of ${label(a, true)} episodes end the day (${c.toLocaleString()})`
            : `${pct(v)} of ${label(a, true)} episodes are followed by ${label(b, false)} (${c.toLocaleString()})`,
    }
  }

  // N-grams per 100 persons, so windows of different sizes compare.
  const grams = $derived.by(() => {
    const per = (q: Counted) => (q.count / Math.max(1, summary.n_persons)) * 100
    const pick = (n: number) =>
      summary.ngrams
        .filter((q) => q.codes.length === n)
        .slice(0, N_GRAMS)
        .map((q) => ({ q, rate: per(q) }))
    return [
      { n: 3, list: pick(3) },
      { n: 4, list: pick(4) },
    ]
  })
  // One scale for every window.
  const rateMax = $derived(
    Math.max(1e-9, ...app.windows.flatMap((w) => (w.summary ? w.summary.ngrams.map((q) => (q.count / Math.max(1, w.summary!.n_persons)) * 100) : []))),
  )
</script>

<div class="matrix">
  <div class="rows" style:width="{LABEL_W}px">
    <div class="corner muted" style:height="{HEAD_H}px"><span>to →</span><span>from ↓</span></div>
    {#each states as s (s)}
      <div class="rl" style:height="{cell}px" title="from {label(s, true)}">
        <span class="sw" style:background={colorOf(s)}></span>{label(s, true)}
      </div>
    {/each}
  </div>
  <div>
    <div class="cols" style:height="{HEAD_H}px">
      {#each states as s (s)}
        <div class="cl" style:width="{cell}px" title="to {label(s, false)}">
          <span class="rot"><span class="sw" style:background={colorOf(s)}></span>{label(s, false)}</span>
        </div>
      {/each}
    </div>
    <div class="plot">
      <canvas
        bind:this={canvas}
        style:width="{states.length * cell}px"
        style:height="{states.length * cell}px"
        aria-label="Transitions between activities"
        onpointermove={onMove}
        onpointerleave={() => (tip = null)}
      ></canvas>
      {#if tip}
        <div class="stip" style:left="{Math.max(0, Math.min(states.length * cell - 200, tip.x + 12))}px" style:top="{tip.y + 12}px">
          <div>{tip.text}</div>
          {#if tip.sub}<div class="muted">{tip.sub}</div>{/if}
        </div>
      {/if}
    </div>
  </div>
</div>
<p class="caption muted">
  Each row shows what follows that activity{showText ? ' (% of the row)' : ''}, coloured by the next activity; stronger colour is a larger share.
  The day start row is the first activity of the day, and the day end column is the last.
</p>

<div class="grams">
  {#each grams as g (g.n)}
    <div>
      <div class="muted small">Most common {g.n}-activity chains, per 100 persons</div>
      <ol>
        {#each { length: N_GRAMS } as _, i (i)}
          {@const x = g.list[i]}
          <li>
            {#if x}
              <span class="chain" title={x.q.codes.map((c) => app.activities[c]?.label).join(' → ')}>
                {#each x.q.codes as c, k (k)}
                  {#if k}<span class="arrow muted">›</span>{/if}<span class="sw" style:background={app.activities[c]?.color}></span><span class="name">{app.activities[c]?.label}</span>
                {/each}
              </span>
              <span class="rbar"><span style:width="{(x.rate / rateMax) * 100}%"></span></span>
              <span class="num">{x.rate.toFixed(x.rate < 10 ? 1 : 0)}</span>
            {/if}
          </li>
        {/each}
      </ol>
    </div>
  {/each}
</div>

<style>
  .matrix {
    display: flex;
    font-size: 11px;
  }
  .corner {
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    align-items: flex-end;
    padding: 0 6px 4px 0;
    font-size: 10px;
  }
  .rl {
    display: flex;
    align-items: center;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    padding-right: 6px;
  }
  .cols {
    display: flex;
  }
  .cl {
    position: relative;
  }
  /* Column labels, rotated to fit above narrow cells. */
  .rot {
    position: absolute;
    left: 50%;
    bottom: 4px;
    transform-origin: left bottom;
    transform: rotate(-55deg);
    white-space: nowrap;
  }
  .plot {
    position: relative;
  }
  canvas {
    display: block;
  }
  .grams {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
    gap: 12px;
    margin-top: 12px;
  }
  ol {
    list-style: none;
    padding: 0;
    margin: 4px 0 0;
  }
  li {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 18px;
    font-size: 11px;
  }
  .chain {
    display: inline-flex;
    align-items: center;
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
  }
  .chain .sw {
    margin-right: 2px;
    flex: none;
  }
  .arrow {
    margin: 0 3px;
  }
  .rbar {
    flex: 1 0 40px;
    height: 6px;
    background: var(--chip);
    border-radius: 3px;
    overflow: hidden;
  }
  .rbar span {
    display: block;
    height: 100%;
    background: var(--muted);
  }
  .num {
    font-variant-numeric: tabular-nums;
    width: 30px;
    text-align: right;
  }
</style>
