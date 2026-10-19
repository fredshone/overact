// Several datasets side by side: each has its own engine, and windows pick
// one. Things shared by all windows (the sort, the legend) refer to a union
// header of the loaded datasets and are translated to each dataset by name.

import type { ColumnMeta, Condition, Header, SortKey } from './engine/types'
import { nameCond, nameSort, resolveCond, resolveSort } from './viewconfig'

/**
 * Union of the datasets' headers: activities and columns matched by name, in
 * order of first appearance (so the first dataset's codes and column indices
 * are kept). A single header is returned as is.
 */
export function unionHeader(hs: Header[]): Header {
  if (hs.length === 1) return hs[0]
  const labels: string[] = []
  const columns: ColumnMeta[] = []
  const byName = new Map<string, ColumnMeta>()
  for (const h of hs) {
    for (const a of h.activities) if (!labels.includes(a.label)) labels.push(a.label)
    for (const c of h.columns) {
      const u = byName.get(c.name)
      if (!u) {
        const copy = { ...c, categories: c.categories ? [...c.categories] : undefined }
        byName.set(c.name, copy)
        columns.push(copy)
        continue
      }
      if (u.kind !== c.kind) continue
      u.min = u.min === null ? c.min : c.min === null ? u.min : Math.min(u.min, c.min)
      u.max = u.max === null ? c.max : c.max === null ? u.max : Math.max(u.max, c.max)
      u.nullCount = (u.nullCount ?? 0) + (c.nullCount ?? 0)
      for (const cat of c.categories ?? []) if (!u.categories!.includes(cat)) u.categories!.push(cat)
    }
  }
  return {
    ...hs[0],
    n_persons: hs.reduce((s, h) => s + h.n_persons, 0),
    n_segments: hs.reduce((s, h) => s + h.n_segments, 0),
    activities: labels.map((label, code) => ({ code, label })),
    columns,
    meta: {},
  }
}

/** The union code of each of `h`'s activity codes. */
export function toUnionCodes(h: Header, union: Header): number[] {
  return h.activities.map((a) => union.activities.findIndex((u) => u.label === a.label))
}

/** A profile (`count[code * L + minute]`) with codes moved to the union's. */
export function remapProfile(p: Uint32Array, toUnion: number[], nUnion: number, L: number): Uint32Array {
  if (toUnion.length === nUnion && toUnion.every((u, c) => u === c)) return p
  const out = new Uint32Array(nUnion * L)
  toUnion.forEach((u, c) => out.set(p.subarray(c * L, (c + 1) * L), u * L))
  return out
}

/**
 * Sort keys (of header `from`) for header `to`. Keys whose column or
 * activity is not in `to` are left out; `missing` lists their indices.
 */
export function translateSort(from: Header, to: Header, keys: SortKey[]): { keys: SortKey[]; missing: number[] } {
  if (from === to) return { keys, missing: [] }
  const out: SortKey[] = []
  const missing: number[] = []
  keys.forEach((k, i) => {
    try {
      out.push(resolveSort(to, nameSort(from, k)))
    } catch {
      missing.push(i)
    }
  })
  return { keys: out, missing }
}

/** Filter conditions (of header `from`) for header `to`; ones that do not fit are dropped. */
export function translateFilter(from: Header, to: Header, filter: Condition[]): { filter: Condition[]; dropped: number } {
  if (from === to) return { filter, dropped: 0 }
  const out: Condition[] = []
  for (const c of filter) {
    try {
      out.push(resolveCond(to, nameCond(from, c)))
    } catch {
      /* not in `to` */
    }
  }
  return { filter: out, dropped: filter.length - out.length }
}

/** Why `h` cannot be compared with `ref`, or null. */
export function incompatibility(ref: Header, h: Header): string | null {
  const a = ref.time_axis
  const b = h.time_axis
  if (a.start !== b.start || a.end !== b.end) return 'its time axis differs from the loaded dataset’s'
  return null
}
