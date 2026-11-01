// Helpers for the Summary mode charts: moving a summary to union activity
// codes, and scales shared by all windows so they compare honestly.

import type { Counted, Summary } from './engine/types'

/** Last bin of the episodes-per-activity counts ("or more"); `MAX_EPISODES` in Rust. */
export const MAX_EPISODES_BIN = 6

/** A summary with activity codes moved to the union's (see `toUnionCodes`). */
export function remapSummary(s: Summary, toUnion: number[], nUnion: number): Summary {
  const n = s.n_activities
  if (n === nUnion && toUnion.every((u, c) => u === c)) return s
  const blocks = (a: number[], size: number) => {
    const out = new Array<number>(nUnion * size).fill(0)
    for (let c = 0; c < n; c++) for (let k = 0; k < size; k++) out[toUnion[c] * size + k] = a[c * size + k]
    return out
  }
  // Transition states: codes, then start/end of day.
  const state = (c: number) => (c === n ? nUnion : toUnion[c])
  const trans = new Array<number>((nUnion + 1) ** 2).fill(0)
  for (let a = 0; a <= n; a++) for (let b = 0; b <= n; b++) trans[state(a) * (nUnion + 1) + state(b)] = s.trans[a * (n + 1) + b]
  const codes = (l: Counted[]) => l.map((q) => ({ codes: q.codes.map((c) => toUnion[c]), count: q.count }))
  return {
    ...s,
    n_activities: nUnion,
    sequences: codes(s.sequences),
    ngrams: codes(s.ngrams),
    tail: tailCodes(s.tail, s.tail_rows * s.tail_cols, n, nUnion, toUnion),
    start_dur: blocks(s.start_dur, s.bins * s.bins),
    episodes: blocks(s.episodes, MAX_EPISODES_BIN + 1),
    minutes: blocks(s.minutes, s.bins),
    trans,
  }
}

/** A `[cell][code]` array with codes moved to the union's. */
function tailCodes(a: number[], cells: number, n: number, nUnion: number, toUnion: number[]): number[] {
  const out = new Array<number>(cells * nUnion).fill(0)
  for (let k = 0; k < cells; k++) for (let c = 0; c < n; c++) out[k * nUnion + toUnion[c]] = a[k * n + c]
  return out
}

/** Episodes of activity `a` (sum of its start × duration block). */
export function episodeCount(s: Summary, a: number): number {
  const B = s.bins * s.bins
  let t = 0
  for (let i = a * B; i < (a + 1) * B; i++) t += s.start_dur[i]
  return t
}

export type DensityView = 'density' | 'starts' | 'durations'

/**
 * Values of one activity's start × duration chart, as shares of its
 * episodes: a `bins × bins` grid (`[start][duration]`), or a histogram of
 * starts or durations (`bins` values).
 */
export function densityValues(s: Summary, a: number, view: DensityView): Float64Array {
  const n = s.bins
  const base = a * n * n
  const total = episodeCount(s, a)
  const out = new Float64Array(view === 'density' ? n * n : n)
  if (!total) return out
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < n; j++) {
      const v = s.start_dur[base + i * n + j] / total
      if (view === 'density') out[i * n + j] = v
      else out[view === 'starts' ? i : j] += v
    }
  }
  return out
}

/**
 * The largest density value of each activity over all the summaries, so
 * every window shades an activity on the same scale.
 */
export function sharedMax(summaries: Summary[], nAct: number, view: DensityView): number[] {
  const max = new Array<number>(nAct).fill(0)
  for (const s of summaries) {
    for (let a = 0; a < nAct && a < s.n_activities; a++) {
      const v = densityValues(s, a, view)
      for (let i = 0; i < v.length; i++) if (v[i] > max[a]) max[a] = v[i]
    }
  }
  return max
}

/** Share of persons doing activity `a` at least once. */
export function participation(s: Summary, a: number): number {
  return s.n_persons ? 1 - s.episodes[a * (MAX_EPISODES_BIN + 1)] / s.n_persons : 0
}

/** Mean total minutes in activity `a` among those doing it (bin midpoints). */
export function meanMinutes(s: Summary, a: number): number {
  let n = 0
  let t = 0
  for (let k = 0; k < s.bins; k++) {
    const c = s.minutes[a * s.bins + k]
    n += c
    t += c * (k + 0.5) * s.bin
  }
  return n ? t / n : 0
}

/** Transition shares from each state (rows sum to 1, or 0 if never left). */
export function transitionShares(s: Summary): Float64Array {
  const n = s.n_activities + 1
  const out = new Float64Array(n * n)
  for (let a = 0; a < n; a++) {
    let row = 0
    for (let b = 0; b < n; b++) row += s.trans[a * n + b]
    if (row) for (let b = 0; b < n; b++) out[a * n + b] = s.trans[a * n + b] / row
  }
  return out
}

/** A tick at fraction `f` (0..1) along an axis. */
export interface Tick {
  f: number
  label: string
}

/** Minutes as a short duration label: "0", "30m", "6h", "1h30". */
function short(m: number): string {
  if (m === 0) return '0'
  const h = Math.floor(m / 60)
  const r = m % 60
  return h ? (r ? `${h}h${String(r).padStart(2, '0')}` : `${h}h`) : `${r}m`
}

/** A round step (in minutes) giving about `n` ticks over `span` minutes. */
function step(span: number, n: number): number {
  const steps = [15, 30, 60, 120, 180, 240, 360, 480, 720, 1440]
  return steps.find((s) => span / s <= n) ?? 1440
}

/**
 * Ticks for times of day over an axis of `len` minutes starting at minute
 * `start` of the day (whole hours, labelled "06", "12", ...).
 */
export function timeTicks(start: number, len: number, n = 4): Tick[] {
  const s = step(len, n)
  const out: Tick[] = []
  for (let t = Math.ceil(start / s) * s; t <= start + len; t += s) {
    const h = Math.floor(t / 60)
    const m = t % 60
    out.push({ f: (t - start) / len, label: m ? `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}` : String(h).padStart(2, '0') })
  }
  return out
}

/** Ticks for durations from 0 to `max` minutes. */
export function durationTicks(max: number, n = 4): Tick[] {
  const s = step(max, n)
  const out: Tick[] = []
  for (let t = 0; t <= max; t += s) out.push({ f: t / max, label: short(t) })
  return out
}

/** Ticks for shares from 0 to `max` (e.g. 0%, 10%, 20%). */
export function shareTicks(max: number, n = 3): Tick[] {
  if (!(max > 0)) return []
  const pcts = [0.1, 0.2, 0.5, 1, 2, 5, 10, 20, 25, 50, 100]
  const s = pcts.find((p) => (max * 100) / p <= n) ?? 100
  const out: Tick[] = []
  for (let k = 0; k * s <= max * 100 + 1e-9; k++) out.push({ f: (k * s) / (max * 100), label: `${+(k * s).toFixed(1)}%` })
  return out
}
