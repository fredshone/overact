// ViewConfig: everything about a view except the data, so it can be saved
// to a file or put in the URL hash. Columns and activities are stored by
// name, so a config also applies to another dataset with the same columns.

import type { ColRef, Condition, Feature, Header, LegacyFeature, RenderMode, SortKey } from './engine/types'
import { isFeature, normalizeFeature } from './format'

type NamedCol = { column: string } | (Record<string, unknown> & { feature: string; activity?: string })

export interface ViewConfig {
  overact: 1
  /** The first dataset (the only one in older configs). */
  dataset: { name: string; persons: number; fingerprint: string }
  /** All datasets, when there are several; windows refer to them by index. */
  datasets?: { name: string; persons: number; fingerprint: string }[]
  crop?: { on: boolean; start: number; end: number }
  colors: Record<string, string>
  hidden: string[]
  vertical?: { mode: 'fit' | 'rows'; pos?: number }
  show?: number | null
  sample?: { n: number | null; seed: number }
  renderMode: RenderMode
  maxSamples: number
  profileHeight?: number
  sort?: NamedSortKey[]
  windows: { title: string; filter: NamedCondition[]; sort?: NamedSortKey[]; dataset?: number }[]
  // Older configs: a zoomed time range, and per-window sorts that could be linked.
  time?: { t0: number; t1: number }
  linkSort?: boolean
  sharedSort?: NamedSortKey[]
}

// Category indices differ between datasets, so conditions and custom orders
// on categorical columns also carry the category labels (`values` / `order`
// are kept for older readers; labels win when present).
export type NamedCondition = Omit<Condition, 'col'> & { col: NamedCol; categories?: string[] }
type NamedSortKey = Omit<SortKey, 'col'> & { col: NamedCol; orderCategories?: string[] }

/** Short stable hash of the dataset's shape (not its data). */
export function fingerprint(h: Header): string {
  const s = JSON.stringify([
    h.n_persons,
    h.n_segments,
    h.time_axis.start,
    h.time_axis.end,
    h.activities.map((a) => a.label),
    h.columns.map((c) => [c.name, c.kind, c.dtype]),
  ])
  let x = 0x811c9dc5
  for (let i = 0; i < s.length; i++) x = Math.imul(x ^ s.charCodeAt(i), 0x01000193)
  return (x >>> 0).toString(16).padStart(8, '0')
}

export function nameCol(h: Header, c: ColRef): NamedCol {
  if (!isFeature(c)) return { column: h.columns[c.column].name }
  const out: Record<string, unknown> = { ...c }
  if ('activity' in c && c.activity !== null) out.activity = h.activities[c.activity].label
  return out as NamedCol
}

/** Resolves a named column against a header; throws with a readable message. */
export function resolveCol(h: Header, c: NamedCol): ColRef {
  if ('column' in c) {
    const i = h.columns.findIndex((m) => m.name === c.column)
    if (i < 0) throw new Error(`column "${c.column}" is not in this dataset`)
    return { column: i }
  }
  const out: Record<string, unknown> = { ...c }
  if (typeof c.activity === 'string') {
    const code = h.activities.findIndex((a) => a.label === c.activity)
    if (code < 0) throw new Error(`activity "${c.activity}" is not in this dataset`)
    out.activity = code
  }
  return normalizeFeature(out as Feature | LegacyFeature)
}

/** Category labels of a categorical attribute column, else null. */
function categoriesOf(h: Header, c: ColRef): string[] | null {
  if (isFeature(c)) return null
  const m = h.columns[c.column]
  return m?.kind === 'categorical' ? (m.categories ?? []) : null
}

export function nameCond(h: Header, c: Condition): NamedCondition {
  const out: NamedCondition = { ...c, col: nameCol(h, c.col) }
  const cats = categoriesOf(h, c.col)
  if (c.type === 'in' && cats) out.categories = c.values.map((v) => cats[v])
  return out
}

export function nameSort(h: Header, k: SortKey): NamedSortKey {
  const out: NamedSortKey = { ...k, col: nameCol(h, k.col) }
  const cats = categoriesOf(h, k.col)
  if (k.order && cats) {
    // Labels from first to last rank.
    out.orderCategories = cats
      .map((label, i) => ({ label, rank: k.order![i] ?? Infinity }))
      .sort((a, b) => a.rank - b.rank)
      .map((x) => x.label)
  }
  return out
}

export function resolveCond(h: Header, c: NamedCondition): Condition {
  const { categories, ...rest } = c
  const col = resolveCol(h, c.col)
  const out = { ...rest, col } as Condition
  const cats = categoriesOf(h, col)
  if (out.type === 'in' && cats && Array.isArray(categories)) {
    out.values = categories
      .map((label) => {
        const i = cats.indexOf(label)
        if (i < 0) throw new Error(`category "${label}" of "${h.columns[(col as { column: number }).column].name}" is not in this dataset`)
        return i
      })
      .sort((a, b) => a - b)
  }
  return out
}

export function resolveSort(h: Header, k: NamedSortKey): SortKey {
  const { orderCategories, ...rest } = k
  const col = resolveCol(h, k.col)
  const out: SortKey = { ...rest, col }
  const cats = categoriesOf(h, col)
  if (cats && Array.isArray(orderCategories)) {
    // Categories missing from the saved order go last, in their own order.
    out.order = cats.map((label, i) => {
      const r = orderCategories.indexOf(label)
      return r < 0 ? orderCategories.length + i : r
    })
  }
  return out
}

export function isViewConfig(x: unknown): x is ViewConfig {
  return typeof x === 'object' && x !== null && (x as ViewConfig).overact === 1 && Array.isArray((x as ViewConfig).windows)
}

// --- URL hash: base64url of the JSON, only when small ---

export const MAX_HASH = 4000

export function encodeHash(cfg: ViewConfig): string | null {
  const json = JSON.stringify(cfg)
  const b64 = btoa(String.fromCharCode(...new TextEncoder().encode(json)))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '')
  return b64.length <= MAX_HASH ? `#view=${b64}` : null
}

export function decodeHash(hash: string): ViewConfig | null {
  const m = /[#&]view=([A-Za-z0-9_-]+)/.exec(hash)
  if (!m) return null
  try {
    const b64 = m[1].replace(/-/g, '+').replace(/_/g, '/')
    const bytes = Uint8Array.from(atob(b64), (ch) => ch.charCodeAt(0))
    const cfg = JSON.parse(new TextDecoder().decode(bytes))
    return isViewConfig(cfg) ? cfg : null
  } catch {
    return null
  }
}
