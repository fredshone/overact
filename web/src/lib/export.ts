// Exports: PNG of the whole view or one window, SVG of the legend and
// profile strips, person ids (CSV / Parquet), the view config, and the
// dataset itself as an .overact file.

import { app, isColor, type Dataset, type Win } from './state.svelte'
import { clock, count, describeCondition, describeSort, pct, TICK_STEPS } from './format'

export function download(blob: Blob, name: string) {
  const a = document.createElement('a')
  a.href = URL.createObjectURL(blob)
  a.download = name
  document.body.appendChild(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(a.href), 30_000)
}

export function safeName(s: string): string {
  return s.replace(/[^\w.-]+/g, '_').replace(/^_+|_+$/g, '') || 'overact'
}

function stamp(): string {
  return new Date().toISOString().slice(0, 16).replace(/[:T]/g, '-')
}

export async function exportIds(w: Win, format: 'csv' | 'parquet') {
  const blob = await app.datasetOf(w).client.exportIds(w.id, format)
  download(blob, `${safeName(w.title)}_ids.${format}`)
}

/** A loaded dataset (or subset) as an `.overact` file, to skip parsing next time. */
export async function saveOveract(ds: Dataset = app.datasets[0]) {
  const blob = await ds.client.saveOveract()
  const base = ds.name.replace(/\.(csv|tsv|tab|txt|parquet|pq|overact)$/i, '')
  download(blob, `${safeName(base)}.overact`)
}

export function saveConfig() {
  const cfg = app.toConfig()
  download(new Blob([JSON.stringify(cfg, null, 2)], { type: 'application/json' }), `${safeName(viewName())}_view.json`)
}

/** The loaded datasets' names, for file names and titles. */
function viewName(): string {
  return app.datasets.map((d) => d.name.replace(/\.overact$/, '')).join('_vs_')
}

function css(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}

/** Wraps `text` to `width` px; returns lines. */
function wrap(ctx: CanvasRenderingContext2D, text: string, width: number): string[] {
  const words = text.split(' ')
  const lines: string[] = []
  let line = ''
  for (const w of words) {
    const t = line ? `${line} ${w}` : w
    if (ctx.measureText(t).width > width && line) {
      lines.push(line)
      line = w
    } else line = t
  }
  if (line) lines.push(line)
  return lines
}

/**
 * PNG of every window (or one), with titles, counts, filter and sort
 * summaries, profile strips, sort-key gutters and a legend.
 */
export async function exportPNG(only?: Win) {
  const h = app.header!
  const wins = only ? [only] : app.windows
  const els = wins.map((w) => document.querySelector<HTMLElement>(`[data-win="${w.id}"]`))
  const bitmaps = await Promise.all(wins.map((w) => app.datasetOf(w).client.snapshot(w.id)))
  const several = app.datasets.length > 1
  const dpr = bitmaps.find((b) => b)?.width && els[0] ? bitmaps.find((b) => b)!.width / els[0].querySelector<HTMLElement>('.body')!.clientWidth : 1
  const pad = 12 * dpr
  const gutterW = 10 * dpr
  const colW = Math.max(...bitmaps.map((b) => (b?.width ?? 400 * dpr) + gutterW))
  const W = wins.length * colW + (wins.length + 1) * pad
  const font = (px: number, bold = false) => `${bold ? '600 ' : ''}${px * dpr}px system-ui, sans-serif`

  // Measure text blocks per window.
  const measure = document.createElement('canvas').getContext('2d')!
  measure.font = font(12)
  const texts = wins.map((w) => {
    const f = w.filter.map((c) => describeCondition(app.datasetOf(w).header, c)).join(' AND ') || 'all persons'
    const s = app.sort.map((k) => describeSort(h, k)).join(', ') || 'file order'
    return [...wrap(measure, `Filter: ${f}`, colW - gutterW), ...wrap(measure, `Sort: ${s}`, colW - gutterW)]
  })
  const lineH = 16 * dpr
  const headH = 22 * dpr + Math.max(...texts.map((t) => t.length)) * lineH + 6 * dpr
  const stripH = app.profileHeight * dpr
  const axisH = 16 * dpr
  const bodyH = Math.max(...bitmaps.map((b) => b?.height ?? 300 * dpr))
  const legendH = 28 * dpr
  const titleH = 26 * dpr
  const H = pad + titleH + legendH + headH + stripH + axisH + 4 * dpr + bodyH + pad

  const canvas = document.createElement('canvas')
  canvas.width = Math.round(W)
  canvas.height = Math.round(H)
  const ctx = canvas.getContext('2d')!
  ctx.fillStyle = css('--panel') || '#fff'
  ctx.fillRect(0, 0, W, H)
  const fg = css('--fg') || '#000'
  const muted = css('--muted') || '#666'
  ctx.textBaseline = 'top'

  // Title and legend.
  ctx.fillStyle = fg
  ctx.font = font(15, true)
  ctx.fillText(app.datasets.map((d) => `${d.name} · ${count(d.header.n_persons)} persons`).join('  vs  '), pad, pad)
  ctx.font = font(11)
  ctx.fillStyle = muted
  const t0 = clock(h.time_axis.start + app.time.t0)
  const t1 = clock(h.time_axis.start + app.time.t1)
  const extra = [app.sample.n ? `sample ${count(app.sample.n)}` : '', app.show !== null ? `show ${count(app.show)}` : ''].filter(Boolean)
  const right = [`${t0}–${t1}`, app.vertical.mode === 'fit' ? 'Fit' : 'Rows', ...extra, `${['mean', 'majority', 'sample'][app.renderMode]} colour`, 'overact'].join(' · ')
  ctx.fillText(right, W - pad - ctx.measureText(right).width, pad + 3 * dpr)
  let x = pad
  const ly = pad + titleH
  ctx.font = font(12)
  for (const a of app.activities.filter((a) => a.visible)) {
    ctx.fillStyle = a.color
    ctx.fillRect(x, ly + 2 * dpr, 12 * dpr, 12 * dpr)
    ctx.fillStyle = fg
    ctx.fillText(a.label, x + 16 * dpr, ly + 1 * dpr)
    x += 16 * dpr + ctx.measureText(a.label).width + 14 * dpr
  }

  wins.forEach((w, i) => {
    const x0 = pad + i * (colW + pad)
    let y = pad + titleH + legendH
    ctx.fillStyle = fg
    ctx.font = font(13, true)
    ctx.fillText(several ? `${w.title} (${app.datasetOf(w).name})` : w.title, x0, y)
    ctx.font = font(12)
    // The share is of persons matching the filter (before any sample), as in the window header.
    const c = `${w.count?.toLocaleString() ?? '?'} persons (${pct((w.matched ?? 0) / app.datasetOf(w).header.n_persons)})`
    ctx.fillStyle = muted
    ctx.fillText(c, x0 + colW - ctx.measureText(c).width - gutterW, y)
    y += 22 * dpr
    for (const line of texts[i]) {
      ctx.fillText(line, x0, y)
      y += lineH
    }
    y = pad + titleH + legendH + headH
    const el = els[i]
    const strip = el?.querySelector<HTMLCanvasElement>('canvas.strip')
    if (strip) ctx.drawImage(strip, x0 + gutterW, y, colW - gutterW, stripH)
    y += stripH
    drawTicks(ctx, x0 + gutterW, y, colW - gutterW, axisH, dpr, muted)
    y += axisH + 4 * dpr
    const gutter = el?.querySelector<HTMLCanvasElement>('.gutter canvas')
    if (gutter && gutter.height) {
      ctx.imageSmoothingEnabled = false
      ctx.drawImage(gutter, x0, y, gutterW - 2 * dpr, bodyH)
    }
    const bm = bitmaps[i]
    if (bm) ctx.drawImage(bm, x0 + gutterW, y)
  })

  const blob = await new Promise<Blob | null>((r) => canvas.toBlob(r, 'image/png'))
  if (blob) download(blob, `${safeName(only?.title ?? viewName())}_${stamp()}.png`)
}

/** Clock ticks for the visible time range, as in the app's time axis. */
function drawTicks(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, dpr: number, color: string) {
  const a0 = app.header!.time_axis.start
  const { t0, t1 } = app.time
  const span = t1 - t0
  const step = TICK_STEPS.find((s) => (s / span) * w >= 56 * dpr) ?? 720
  ctx.save()
  ctx.fillStyle = color
  ctx.font = `${10 * dpr}px system-ui, sans-serif`
  ctx.textBaseline = 'top'
  for (let m = Math.ceil((t0 + a0) / step) * step - a0; m <= t1; m += step) {
    const tx = x + ((m - t0) / span) * w
    ctx.fillRect(Math.round(tx), y, Math.max(1, Math.round(dpr)), h * 0.4)
    ctx.fillText(clock(a0 + m), tx + 3 * dpr, y + 2 * dpr)
  }
  ctx.restore()
}

function esc(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
}

/** A colour for an SVG attribute (colours can come from shared links). */
function svgColor(c: string): string {
  return isColor(c) ? c : '#808080'
}

/** SVG with the legend and each window's profile (exact activity shares by minute). */
export function exportSVG() {
  const h = app.header!
  const L = app.axisLen
  const { t0, t1 } = app.time
  const W = 640
  const stripH = 120
  const acts = app.activities.filter((a) => a.visible)
  const legendH = 24
  const blockH = stripH + 40
  const Ht = legendH + app.windows.length * blockH + 10
  const xOf = (m: number) => +(((m - t0) / (t1 - t0)) * W).toFixed(1)
  const parts: string[] = []
  parts.push(`<svg xmlns="http://www.w3.org/2000/svg" width="${W + 20}" height="${Ht}" viewBox="0 0 ${W + 20} ${Ht}" font-family="system-ui, sans-serif" font-size="11">`)
  let lx = 10
  for (const a of acts) {
    parts.push(`<rect x="${lx}" y="6" width="10" height="10" fill="${svgColor(a.color)}"/><text x="${lx + 14}" y="15">${esc(a.label)}</text>`)
    lx += 14 + a.label.length * 6.5 + 12
  }
  app.windows.forEach((w, i) => {
    const y0 = legendH + i * blockH
    parts.push(`<g transform="translate(10 ${y0})">`)
    const name = app.datasets.length > 1 ? `${w.title}, ${app.datasetOf(w).name}` : w.title
    parts.push(`<text y="12" font-weight="600">${esc(name)} (${w.count?.toLocaleString() ?? '?'} persons)</text>`)
    const p = w.profile
    const n = w.count ?? 0
    if (p && n) {
      const m0 = Math.max(0, Math.floor(t0))
      const m1 = Math.min(L, Math.ceil(t1))
      const base = new Float64Array(m1 - m0)
      for (const a of acts) {
        const top: string[] = []
        const bottom: string[] = []
        let any = false
        for (let m = m0; m < m1; m++) {
          const b = base[m - m0]
          const v = b + p[a.code * L + m] / n
          base[m - m0] = v
          if (v > b) any = true
          // One point per minute (at its centre), 0.1 px precision.
          const x = xOf(m + 0.5)
          // Stacked from the top, as in the app.
          top.push(`${x},${+(20 + b * stripH).toFixed(1)}`)
          bottom.push(`${x},${+(20 + v * stripH).toFixed(1)}`)
        }
        if (any) {
          bottom.reverse()
          parts.push(`<polygon fill="${svgColor(a.color)}" points="${top.join(' ')} ${bottom.join(' ')}"><title>${esc(a.label)}</title></polygon>`)
        }
      }
    }
    // Hour ticks.
    const step = t1 - t0 > 720 ? 180 : 60
    for (let m = Math.ceil((t0 + h.time_axis.start) / step) * step - h.time_axis.start; m <= t1; m += step) {
      parts.push(`<line x1="${xOf(m)}" x2="${xOf(m)}" y1="${20 + stripH}" y2="${24 + stripH}" stroke="#666"/><text x="${xOf(m)}" y="${35 + stripH}" text-anchor="middle" fill="#666">${clock(h.time_axis.start + m)}</text>`)
    }
    parts.push('</g>')
  })
  parts.push('</svg>')
  download(new Blob([parts.join('\n')], { type: 'image/svg+xml' }), `${safeName(viewName())}_profiles.svg`)
}
