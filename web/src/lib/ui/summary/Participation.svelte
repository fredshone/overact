<script lang="ts">
  import { app } from '../../state.svelte'
  import type { Summary } from '../../engine/types'
  import { duration, pct } from '../../format'
  import { mix } from '../../palette'
  import { MAX_EPISODES_BIN, durationTicks, meanMinutes, participation, shareTicks } from '../../summary'

  let { summary, width }: { summary: Summary; width: number } = $props()
  const acts = $derived(app.activities.filter((a) => a.visible))
  const E = MAX_EPISODES_BIN + 1

  /** Mean episodes per participant (the last bin counted as its minimum). */
  function meanEpisodes(a: number): number {
    let n = 0
    let t = 0
    for (let k = 1; k < E; k++) {
      const c = summary.episodes[a * E + k]
      n += c
      t += c * k
    }
    return n ? t / n : 0
  }

  // Total minutes of participants, as a small histogram per activity (its
  // own vertical scale), with the mean marked.
  const SPARK_H = 22
  const sparkW = $derived(Math.max(60, Math.min(240, Math.floor(width * 0.35))))
  const dpr = $derived(app.dpr)
  const span = $derived(summary.bins * summary.bin)
  const ticks = $derived(durationTicks(span))
  function spark(canvas: HTMLCanvasElement, code: number) {
    $effect(() => {
      const _t = app.redrawToken
      void _t
      const W = Math.round(sparkW * dpr)
      const H = Math.round(SPARK_H * dpr)
      canvas.width = W
      canvas.height = H
      const ctx = canvas.getContext('2d')!
      ctx.clearRect(0, 0, W, H)
      ctx.fillStyle = mix(app.colors.canvasBg, app.colors.fg, 0.1)
      for (const t of ticks) ctx.fillRect(Math.min(W - 1, Math.round(t.f * W)), 0, 1, H)
      ctx.fillRect(0, H - 1, W, 1)
      const n = summary.bins
      const row = summary.minutes.slice(code * n, (code + 1) * n)
      const max = Math.max(1, ...row)
      ctx.fillStyle = app.activities[code].color
      for (let k = 0; k < n; k++) {
        const h = Math.round((row[k] / max) * H)
        const x0 = Math.round((k / n) * W)
        const x1 = Math.round(((k + 1) / n) * W)
        if (h) ctx.fillRect(x0, H - h, Math.max(1, x1 - x0), h)
      }
      if (participation(summary, code)) {
        ctx.fillStyle = app.colors.fg
        ctx.fillRect(Math.round((meanMinutes(summary, code) / span) * W), 0, Math.max(1, Math.round(dpr)), H)
      }
    })
  }

  let sparkTip = $state<{ code: number; x: number; text: string } | null>(null)
  function onSpark(e: PointerEvent, code: number) {
    const n = summary.bins
    const k = Math.max(0, Math.min(n - 1, Math.floor((e.offsetX / sparkW) * n)))
    const row = summary.minutes.slice(code * n, (code + 1) * n)
    const total = row.reduce((a, b) => a + b, 0) || 1
    const b = summary.bin
    sparkTip = { code, x: e.offsetX, text: `${duration(k * b)}–${duration((k + 1) * b)} in total: ${pct(row[k] / total)} of participants` }
  }

  // Episodes per person: 1, 2, … (the last bar: or more), up to the
  // longest in any window so the bars line up.
  const lengths = $derived.by(() => {
    const n = Math.max(1, summary.n_persons)
    let last = 1
    for (const w of app.windows) {
      const e = w.summary?.n_episodes ?? []
      for (let k = e.length - 1; k > last; k--) if (e[k]) last = k
    }
    return summary.n_episodes.slice(1, last + 1).map((c, i) => ({ k: i + 1, share: c / n, more: i + 2 === summary.n_episodes.length }))
  })
  // One scale for every window.
  const lengthMax = $derived(
    Math.max(1e-9, ...app.windows.flatMap((w) => (w.summary ? w.summary.n_episodes.map((c) => c / Math.max(1, w.summary!.n_persons)) : []))),
  )
  const HIST_H = 70
  const lengthTicks = $derived(shareTicks(lengthMax))
</script>

<table>
  <thead>
    <tr class="muted">
      <th>Activity</th>
      <th>Doing it</th>
      <th title="How long participants spend in the activity in total (each activity has its own vertical scale; the line marks the mean)">
        Time spent per participant
        <span class="scale" style:width="{sparkW}px">
          {#each ticks as t (t.label)}<span style:left="{t.f * 100}%">{t.label}</span>{/each}
        </span>
      </th>
      <th title="Mean among participants">Mean</th>
      <th title="Mean episodes per participant">Episodes</th>
    </tr>
  </thead>
  <tbody>
    {#each acts as a (a.code)}
      {@const p = participation(summary, a.code)}
      <tr>
        <td class="name"><span class="sw" style:background={a.color}></span>{a.label}</td>
        <td>
          <span class="bar" title="{pct(p)} of persons do {a.label} at least once">
            <span class="fill" style:width="{p * 100}%" style:background={a.color}></span>
          </span>
          <span class="num">{pct(p)}</span>
        </td>
        <td class="sparkcell">
          <canvas
            use:spark={a.code}
            style:width="{sparkW}px"
            style:height="{SPARK_H}px"
            aria-label="Time spent in {a.label} by participants"
            onpointermove={(e) => onSpark(e, a.code)}
            onpointerleave={() => (sparkTip = null)}
          ></canvas>
          {#if sparkTip?.code === a.code}
            <div class="stip" style:left="{Math.min(sparkW - 120, sparkTip.x + 10)}px" style:top="{SPARK_H + 4}px">
              <div><b>{a.label}</b></div>
              <div class="muted">{sparkTip.text}</div>
            </div>
          {/if}
        </td>
        <td class="num">{p ? duration(meanMinutes(summary, a.code)) : ''}</td>
        <td class="num">{p ? meanEpisodes(a.code).toFixed(1) : ''}</td>
      </tr>
    {/each}
  </tbody>
</table>

<div class="lengths">
  <div class="muted small">Activities in the day (episodes per person), as a share of persons</div>
  <div class="histwrap">
    <div class="yaxis" style:height="{HIST_H}px">
      {#each lengthTicks as t (t.label)}<span style:bottom="{t.f * HIST_H}px">{t.label}</span>{/each}
    </div>
    <div class="hist" role="list" style:height="{HIST_H + 14}px">
      {#each lengthTicks as t (t.label)}<span class="grid" style:bottom="{14 + t.f * HIST_H}px"></span>{/each}
      {#each lengths as l (l.k)}
        <div class="col" role="listitem" title="{l.k}{l.more ? ' or more' : ''} episodes: {pct(l.share)} of persons">
          <span class="b" style:height="{(l.share / lengthMax) * HIST_H}px"></span>
          <span class="k muted">{l.k}{l.more ? '+' : ''}</span>
        </div>
      {/each}
    </div>
  </div>
</div>

<style>
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
  }
  th {
    text-align: left;
    font-weight: normal;
    padding: 0 6px 4px 0;
  }
  td {
    padding: 2px 6px 2px 0;
    white-space: nowrap;
  }
  .name {
    max-width: 120px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .bar {
    display: inline-block;
    width: 80px;
    height: 8px;
    border-radius: 4px;
    background: var(--chip);
    overflow: hidden;
    vertical-align: middle;
  }
  .fill {
    display: block;
    height: 100%;
  }
  .num {
    font-variant-numeric: tabular-nums;
  }
  canvas {
    display: block;
  }
  th {
    vertical-align: bottom;
  }
  .scale {
    position: relative;
    display: block;
    height: 12px;
    margin-top: 2px;
    font-size: 10px;
  }
  .scale span {
    position: absolute;
    transform: translateX(-50%);
  }
  .scale span:first-child {
    transform: none;
  }
  .sparkcell {
    position: relative;
  }
  .lengths {
    margin-top: 12px;
  }
  .histwrap {
    display: flex;
    gap: 4px;
    margin-top: 6px;
  }
  .yaxis {
    position: relative;
    width: 30px;
    flex: none;
    font-size: 10px;
    color: var(--muted);
  }
  .yaxis span {
    position: absolute;
    right: 0;
    transform: translateY(50%);
    line-height: 1;
  }
  .hist {
    position: relative;
    flex: 1;
    display: flex;
    align-items: stretch;
    gap: 2px;
  }
  .grid {
    position: absolute;
    left: 0;
    right: 0;
    height: 1px;
    background: var(--line);
    opacity: 0.6;
  }
  .col {
    position: relative;
    flex: 1;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    min-width: 0;
  }
  .b {
    display: block;
    position: relative;
    background: var(--muted);
    border-radius: 2px 2px 0 0;
    min-height: 1px;
  }
  .k {
    font-size: 10px;
    text-align: center;
    line-height: 14px;
  }
</style>
