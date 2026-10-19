import { describe, expect, it } from 'vitest'
import { incompatibility, remapProfile, toUnionCodes, translateFilter, translateSort, unionHeader } from '../datasets'
import type { ColumnMeta, Header } from '../engine/types'

const num = (name: string, min: number, max: number): ColumnMeta => ({ name, kind: 'numeric', dtype: 'f32', byteOffset: 0, byteLength: 4, nullValue: null, min, max })
const cat = (name: string, categories: string[]): ColumnMeta => ({ name, kind: 'categorical', dtype: 'u8', byteOffset: 0, byteLength: 1, nullValue: 255, min: 0, max: categories.length - 1, categories })

function header(activities: string[], columns: ColumnMeta[], start = 0): Header {
  return {
    format: 'overact',
    version: 1,
    n_persons: 10,
    n_segments: 20,
    time_axis: { start, end: start + 1440 },
    activities: activities.map((label, code) => ({ code, label })),
    blobs: { offsets: { dtype: 'u32', byteOffset: 0, byteLength: 44 }, segments: { dtype: 'u16', byteOffset: 48, byteLength: 40 } },
    columns,
    meta: {},
  }
}

const a = header(['home', 'work'], [num('age', 1, 80), cat('sex', ['f', 'm'])])
const b = header(['work', 'shop', 'home'], [num('income', 0, 9), num('age', 5, 99), cat('sex', ['m', 'x'])])

describe('datasets', () => {
  it('returns a single header as is', () => {
    expect(unionHeader([a])).toBe(a)
  })

  it('unions activities and columns by name, keeping the first dataset first', () => {
    const u = unionHeader([a, b])
    expect(u.activities.map((x) => x.label)).toEqual(['home', 'work', 'shop'])
    expect(u.columns.map((c) => c.name)).toEqual(['age', 'sex', 'income'])
    expect(u.columns[0]).toMatchObject({ min: 1, max: 99 })
    expect(u.columns[1].categories).toEqual(['f', 'm', 'x'])
    expect(u.n_persons).toBe(20)
    // The inputs are not changed.
    expect(a.columns[1].categories).toEqual(['f', 'm'])
    expect(toUnionCodes(a, u)).toEqual([0, 1])
    expect(toUnionCodes(b, u)).toEqual([1, 2, 0])
  })

  it('moves profile rows to union codes', () => {
    const L = 2
    const p = Uint32Array.from([1, 1, 2, 2, 3, 3]) // work, shop, home
    expect(Array.from(remapProfile(p, [1, 2, 0], 3, L))).toEqual([3, 3, 1, 1, 2, 2])
    expect(remapProfile(p, [0, 1, 2], 3, L)).toBe(p)
    expect(Array.from(remapProfile(Uint32Array.from([5, 6]), [0], 2, 1))).toEqual([5, 0])
  })

  it('translates sort keys by name and reports missing ones', () => {
    const u = unionHeader([a, b])
    const keys = [
      { col: { column: 2 }, desc: false }, // income: not in a
      { col: { column: 1 }, desc: true, order: [2, 0, 1] }, // sex ranks: f 2, m 0, x 1
      { col: { feature: 'doing_at' as const, activity: 2, minute: 60 }, desc: false }, // shop: not in a
      { col: { column: 0 }, desc: false },
    ]
    const ta = translateSort(u, a, keys)
    expect(ta.missing).toEqual([0, 2])
    expect(ta.keys).toEqual([
      { col: { column: 1 }, desc: true, order: [2, 0] },
      { col: { column: 0 }, desc: false },
    ])
    const tb = translateSort(u, b, keys)
    expect(tb.missing).toEqual([])
    expect(tb.keys[0].col).toEqual({ column: 0 })
    expect(tb.keys[1]).toMatchObject({ col: { column: 2 }, order: [0, 1] })
    expect(tb.keys[2].col).toEqual({ feature: 'doing_at', activity: 1, minute: 60 })
    expect(tb.keys[3].col).toEqual({ column: 1 })
    expect(translateSort(a, a, keys).keys).toBe(keys)
  })

  it('translates filters, dropping conditions that do not fit', () => {
    const f = [
      { col: { column: 0 }, negate: false, type: 'range' as const, min: 18, max: null },
      { col: { column: 1 }, negate: true, type: 'in' as const, values: [1] }, // m
      { col: { column: 1 }, negate: false, type: 'in' as const, values: [0] }, // f: not in b
    ]
    const t = translateFilter(a, b, f)
    expect(t.dropped).toBe(1)
    expect(t.filter).toEqual([
      { col: { column: 1 }, negate: false, type: 'range', min: 18, max: null },
      { col: { column: 2 }, negate: true, type: 'in', values: [0] },
    ])
  })

  it('only accepts the same time axis', () => {
    expect(incompatibility(a, b)).toBeNull()
    expect(incompatibility(a, header(['home'], [], 240))).toMatch(/time axis/)
  })
})
