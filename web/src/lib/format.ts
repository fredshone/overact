import type { ColRef, ColumnMeta, Condition, Feature, Header, LegacyFeature, SortKey } from './engine/types'

/** Time-axis tick spacings in minutes; the smallest that leaves room for a label is used. */
export const TICK_STEPS = [1, 2, 5, 10, 15, 30, 60, 120, 180, 240, 360, 720]

/** "HH:MM" for an absolute minute (minutes since midnight of day 0). */
export function clock(abs: number): string {
  if (Math.round(abs) === 1440) return '24:00'
  const day = Math.floor(abs / 1440)
  const m = ((Math.round(abs) % 1440) + 1440) % 1440
  const s = `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`
  return day === 0 ? s : `${s}${day > 0 ? '+' : ''}${day}d`
}

/**
 * Parses a clock time ("8", "08:30", "28:00", "04:00+1d") to minutes from
 * the axis start `axis0`. Times before the start mean the next day (axes
 * may start after midnight, e.g. 04:00-28:00). Plain numbers are minutes.
 */
export function parseClock(s: string, axis0: number): number | null {
  const t = s.trim()
  if (!t) return null
  const m = /^(\d{1,2})(?::(\d{2}))?(?:\s*\+(\d)d)?$/.exec(t)
  if (m) {
    if (Number(m[2] ?? 0) >= 60) return null
    let v = Number(m[1]) * 60 + Number(m[2] ?? 0) - axis0
    if (m[3]) return v + Number(m[3]) * 1440
    while (v < 0) v += 1440
    return v
  }
  const n = Number(t)
  return Number.isFinite(n) ? n : null
}

/** Parses a count like "500", "2,000", "10k" or "1.5M"; empty or "all" is null. */
export function parseCount(s: string): number | null | undefined {
  const t = s.trim().toLowerCase().replace(/[,_\s]/g, '')
  if (!t || t === 'all') return null
  const m = /^(\d+(?:\.\d+)?)([km]?)$/.exec(t)
  if (!m) return undefined
  const v = Math.round(Number(m[1]) * (m[2] === 'k' ? 1e3 : m[2] === 'm' ? 1e6 : 1))
  return v >= 1 ? v : undefined
}

/**
 * A window title not in `taken`: "base_0", or the next free number after
 * an existing "_k" suffix ("Workers" -> "Workers_0", "Workers_0" -> "Workers_1").
 */
export function uniqueTitle(base: string, taken: string[]): string {
  const m = /^(.*)_(\d+)$/.exec(base)
  const stem = m ? m[1] : base
  let k = m ? Number(m[2]) + 1 : 0
  while (taken.includes(`${stem}_${k}`)) k++
  return `${stem}_${k}`
}

export function duration(min: number): string {
  if (!Number.isFinite(min)) return ''
  const h = Math.floor(min / 60)
  const m = Math.round(min % 60)
  return h ? `${h}h${m ? String(m).padStart(2, '0') : ''}` : `${m}m`
}

export function count(n: number): string {
  if (n >= 1e6) return `${(n / 1e6).toFixed(n >= 1e7 ? 1 : 2)}M`
  if (n >= 1e4) return `${(n / 1e3).toFixed(0)}k`
  return n.toLocaleString()
}

export function pct(x: number): string {
  if (!Number.isFinite(x)) return ''
  if (x === 0) return '0%'
  if (x < 0.001) return '<0.1%'
  return `${(x * 100).toFixed(x < 0.1 ? 1 : 0)}%`
}

export function number(v: number): string {
  if (!Number.isFinite(v)) return ''
  if (Number.isInteger(v)) return v.toLocaleString()
  const a = Math.abs(v)
  return v.toLocaleString(undefined, { maximumSignificantDigits: a >= 1000 ? 6 : 4 })
}

export function isFeature(c: ColRef): c is Feature {
  return 'feature' in c
}

type MeasureFeature = Extract<Feature, { feature: 'measure' }>

/**
 * Rewrites older feature names as the measure they stand for, and gives each
 * measure one canonical form (so equal features compare and cache as equal).
 */
export function normalizeFeature(f: Feature | LegacyFeature): Feature {
  const m = (measure: MeasureFeature['measure'], activity: number | null, nth: number): MeasureFeature => ({
    feature: 'measure',
    measure,
    activity,
    not: false,
    nth,
  })
  switch (f.feature) {
    case 'minutes':
      return m('duration', f.activity, 0)
    case 'first_start':
      return m('start', f.activity, 1)
    case 'last_end':
      return m('end', f.activity, -1)
    case 'episodes':
      return m('count', f.activity, 0)
    case 'segments':
      return m('count', null, 0)
    case 'measure':
      // Start/end of all episodes are the first start and the last end.
      if (f.nth === 0 && f.measure === 'start') return { ...f, nth: 1 }
      if (f.nth === 0 && f.measure === 'end') return { ...f, nth: -1 }
      if (f.measure === 'count') return { ...f, nth: 0 }
      return f
    case 'distinct':
    case 'doing_at':
    case 'doing_during':
      return f
  }
}

/** "1st", "2nd", "last", "2nd last". */
export function ordinal(nth: number): string {
  if (nth < 0) return nth === -1 ? 'last' : `${ordinal(-nth)} last`
  const s = nth % 100 >= 11 && nth % 100 <= 13 ? 'th' : (['th', 'st', 'nd', 'rd'][nth % 10] ?? 'th')
  return `${nth}${s}`
}

/** Name of a measure, e.g. "end of 1st episode", "time in non-home". */
export function measureName(h: Header, f: MeasureFeature): string {
  const label = f.activity === null ? '' : (h.activities[f.activity]?.label ?? '?')
  const act = f.activity === null ? '' : f.not ? `non-${label} ` : `${label} `
  if (f.measure === 'count') return `number of ${act}episodes`
  if (f.measure === 'duration' && !f.nth) return f.activity === null ? 'total time' : `time in ${act.trim()}`
  return `${f.measure} of ${ordinal(f.nth || (f.measure === 'end' ? -1 : 1))} ${act}episode`
}

export function sameCol(a: ColRef, b: ColRef): boolean {
  return JSON.stringify(a) === JSON.stringify(b)
}

/** Whether a feature's values are times (minutes from the axis start). */
export function isTimeFeature(f: Feature): boolean {
  const n = normalizeFeature(f)
  return n.feature === 'measure' && (n.measure === 'start' || n.measure === 'end')
}

export function isDurationFeature(f: Feature): boolean {
  const n = normalizeFeature(f)
  return n.feature === 'measure' && n.measure === 'duration'
}

export function colName(h: Header, c: ColRef): string {
  if (!isFeature(c)) return h.columns[c.column]?.name ?? `#${c.column}`
  const f = normalizeFeature(c)
  const t = (m: number) => clock(h.time_axis.start + m)
  switch (f.feature) {
    case 'measure':
      return measureName(h, f)
    case 'doing_at':
      return `doing ${h.activities[f.activity]?.label ?? '?'} at ${t(f.minute)}`
    case 'doing_during':
      return `doing ${h.activities[f.activity]?.label ?? '?'} ${f.all ? 'throughout' : 'during'} ${t(f.start)}–${t(f.end)}`
    default:
      return 'distinct activities'
  }
}

export function colMeta(h: Header, c: ColRef): ColumnMeta | null {
  return isFeature(c) ? null : (h.columns[c.column] ?? null)
}

export function colKind(h: Header, c: ColRef): 'numeric' | 'categorical' | 'boolean' {
  if (isFeature(c)) return c.feature === 'doing_at' || c.feature === 'doing_during' ? 'boolean' : 'numeric'
  return h.columns[c.column]?.kind ?? 'numeric'
}

/** Formats a raw column value for display. */
export function formatValue(h: Header, c: ColRef, v: number | null): string {
  if (v === null || !Number.isFinite(v)) return '—'
  if (isFeature(c)) {
    if (isTimeFeature(c)) return clock(h.time_axis.start + v)
    if (isDurationFeature(c)) return duration(v)
    if (c.feature === 'doing_at' || c.feature === 'doing_during') return v ? 'yes' : 'no'
    return number(v)
  }
  const m = h.columns[c.column]
  if (m.kind === 'categorical') return m.categories?.[v] ?? String(v)
  if (m.kind === 'boolean') return v ? 'true' : 'false'
  return number(v)
}

/** The condition says whether a schedule contains an activity (see the filter picker). */
export function containsTest(c: Condition): { activity: number; not: boolean } | null {
  const f = isFeature(c.col) ? normalizeFeature(c.col) : null
  if (f?.feature !== 'measure' || f.measure !== 'count' || f.activity === null || f.nth) return null
  if (c.type === 'range' && c.min === 1 && c.max === null) return { activity: f.activity, not: f.not }
  return null
}

export function describeCondition(h: Header, c: Condition): string {
  const contains = containsTest(c)
  if (contains) {
    const label = h.activities[contains.activity]?.label ?? '?'
    return `${c.negate ? "doesn't contain" : 'contains'} ${contains.not ? `non-${label}` : label}`
  }
  const name = colName(h, c.col)
  const not = c.negate ? 'not ' : ''
  if (c.type === 'is_null') return `${name} is ${not}missing`
  if (c.type === 'in') {
    const kind = colKind(h, c.col)
    if (kind === 'boolean' && c.values.length === 1 && !c.null) {
      const yes = c.values[0] === 1 !== c.negate
      return isFeature(c.col) ? `${yes ? '' : 'not '}${name}` : `${name} = ${yes}`
    }
    const vals = c.values.map((v) => formatValue(h, c.col, v))
    if (c.null) vals.push('missing')
    const list = vals.length > 3 ? `${vals.slice(0, 3).join(', ')} +${vals.length - 3}` : vals.join(', ')
    return `${name} ${c.negate ? '∉' : '∈'} {${list}}`
  }
  const f = (v: number) => formatValue(h, c.col, v)
  let s: string
  if (c.min !== null && c.max !== null) s = `${f(c.min)} ≤ ${name} ≤ ${f(c.max)}`
  else if (c.min !== null) s = `${name} ≥ ${f(c.min)}`
  else if (c.max !== null) s = `${name} ≤ ${f(c.max)}`
  else s = `${name} present`
  return c.negate ? `not (${s})` : s
}

export function describeSort(h: Header, k: SortKey): string {
  return `${colName(h, k.col)} ${k.desc ? '↓' : '↑'}`
}
