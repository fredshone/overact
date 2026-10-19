import { describe, expect, it } from 'vitest'
import { clock, colName, describeCondition, duration, normalizeFeature, ordinal, parseClock, parseCount, pct, uniqueTitle } from '../format'
import { decodeHash, encodeHash, fingerprint, nameCond, nameSort, resolveCol, nameCol, resolveCond, resolveSort, type ViewConfig } from '../viewconfig'
import { writeParquet } from '../parquet'
import type { Header } from '../engine/types'

const header: Header = {
  format: 'overact',
  version: 1,
  n_persons: 3,
  n_segments: 7,
  time_axis: { start: 240, end: 1680 },
  activities: [
    { code: 0, label: 'home' },
    { code: 1, label: 'work' },
  ],
  blobs: {
    offsets: { dtype: 'u32', byteOffset: 0, byteLength: 16 },
    segments: { dtype: 'u16', byteOffset: 16, byteLength: 14 },
  },
  columns: [{ name: 'age', kind: 'numeric', dtype: 'i8', byteOffset: 32, byteLength: 3, nullValue: -128, min: 1, max: 9 }],
  meta: {},
}

describe('format', () => {
  it('formats clock times across midnight', () => {
    expect(clock(0)).toBe('00:00')
    expect(clock(545)).toBe('09:05')
    expect(clock(1440)).toBe('24:00')
    expect(clock(1500)).toBe('01:00+1d')
  })
  it('parses clock times, next day on shifted axes', () => {
    expect(parseClock('08:30', 0)).toBe(510)
    expect(parseClock('8', 0)).toBe(480)
    expect(parseClock('24:00', 0)).toBe(1440)
    expect(parseClock('03:00', 240)).toBe(1380)
    expect(parseClock('04:00+1d', 240)).toBe(1440)
    expect(parseClock('28:00', 240)).toBe(1440)
    expect(parseClock('8:75', 0)).toBeNull()
    expect(parseClock('', 0)).toBeNull()
  })
  it('parses counts', () => {
    expect(parseCount('2,000')).toBe(2000)
    expect(parseCount('10k')).toBe(10000)
    expect(parseCount('1.5M')).toBe(1500000)
    expect(parseCount('')).toBeNull()
    expect(parseCount('all')).toBeNull()
    expect(parseCount('0')).toBeUndefined()
    expect(parseCount('abc')).toBeUndefined()
  })
  it('makes unique window titles', () => {
    expect(uniqueTitle('Unnamed', [])).toBe('Unnamed_0')
    expect(uniqueTitle('Unnamed', ['Unnamed_0', 'Unnamed_1'])).toBe('Unnamed_2')
    expect(uniqueTitle('Workers', ['Workers'])).toBe('Workers_0')
    expect(uniqueTitle('Workers_0', ['Workers_0'])).toBe('Workers_1')
    expect(uniqueTitle('Workers_0', ['Workers_0', 'Workers_1'])).toBe('Workers_2')
  })
  it('names schedule measures', () => {
    const m = (measure: 'count' | 'start' | 'duration' | 'end', activity: number | null, not: boolean, nth: number) =>
      ({ feature: 'measure', measure, activity, not, nth }) as const
    expect(colName(header, m('end', null, false, 1))).toBe('end of 1st episode')
    expect(colName(header, m('duration', null, false, 2))).toBe('duration of 2nd episode')
    expect(colName(header, m('duration', 0, true, 0))).toBe('time in non-home')
    expect(colName(header, m('start', 1, false, -2))).toBe('start of 2nd last work episode')
    expect(colName(header, m('count', 1, false, 0))).toBe('number of work episodes')
    expect(colName(header, normalizeFeature({ feature: 'minutes', activity: 1 }))).toBe('time in work')
    expect(ordinal(11)).toBe('11th')
    expect(ordinal(22)).toBe('22nd')
  })
  it('describes "contains" filters', () => {
    const col = { feature: 'measure', measure: 'count', activity: 0, not: true, nth: 0 } as const
    expect(describeCondition(header, { col, negate: false, type: 'range', min: 1, max: null })).toBe('contains non-home')
    expect(describeCondition(header, { col: { ...col, not: false }, negate: true, type: 'range', min: 1, max: null })).toBe("doesn't contain home")
    expect(describeCondition(header, { col, negate: false, type: 'range', min: 2, max: null })).toBe('number of non-home episodes ≥ 2')
  })
  it('rewrites older feature names as measures', () => {
    expect(normalizeFeature({ feature: 'first_start', activity: 1 })).toEqual({ feature: 'measure', measure: 'start', activity: 1, not: false, nth: 1 })
    expect(normalizeFeature({ feature: 'segments' })).toEqual({ feature: 'measure', measure: 'count', activity: null, not: false, nth: 0 })
    expect(normalizeFeature({ feature: 'measure', measure: 'end', activity: null, not: false, nth: 0 })).toMatchObject({ nth: -1 })
    // A saved view from before measures existed.
    expect(resolveCol(header, { feature: 'last_end', activity: 'work' })).toEqual({ feature: 'measure', measure: 'end', activity: 1, not: false, nth: -1 })
    const any = { feature: 'measure', measure: 'duration', activity: null, not: false, nth: 2 } as const
    expect(resolveCol(header, nameCol(header, any))).toEqual(any)
  })
  it('formats durations and shares', () => {
    expect(duration(95)).toBe('1h35')
    expect(duration(120)).toBe('2h')
    expect(pct(0.0004)).toBe('<0.1%')
    expect(pct(0.25)).toBe('25%')
  })
})

describe('viewconfig', () => {
  it('names and resolves columns and activities', () => {
    const f = { feature: 'doing_at', activity: 1, minute: 60 } as const
    expect(nameCol(header, f)).toEqual({ feature: 'doing_at', activity: 'work', minute: 60 })
    expect(resolveCol(header, nameCol(header, f))).toEqual(f)
    expect(resolveCol(header, { column: 'age' })).toEqual({ column: 0 })
    expect(() => resolveCol(header, { column: 'nope' })).toThrow(/nope/)
  })
  it('round-trips through the URL hash', () => {
    const cfg: ViewConfig = {
      overact: 1,
      dataset: { name: 'x', persons: 3, fingerprint: fingerprint(header) },
      crop: { on: true, start: 60, end: 1200 },
      colors: { home: '#ffffff', work: '#000000' },
      hidden: ['home'],
      vertical: { mode: 'fit', pos: 0.5 },
      show: 50,
      sample: { n: 1000, seed: 2 },
      renderMode: 0,
      maxSamples: 32,
      profileHeight: 108,
      sort: [{ col: { column: 'age' }, desc: true }],
      windows: [{ title: 'Ünïcode ✓', filter: [] }],
    }
    const hash = encodeHash(cfg)!
    expect(hash.startsWith('#view=')).toBe(true)
    expect(decodeHash(hash)).toEqual(cfg)
    expect(decodeHash('#view=not-base64!')).toBeNull()
  })
  it('maps categories by label, not index', () => {
    const cat = (cats: string[]): Header => ({
      ...header,
      columns: [{ name: 'mode', kind: 'categorical', dtype: 'u8', byteOffset: 32, byteLength: 3, nullValue: 255, min: 0, max: cats.length - 1, categories: cats }],
    })
    const a = cat(['bike', 'car', 'walk'])
    const b = cat(['bus', 'car', 'train', 'walk'])
    const named = nameCond(a, { col: { column: 0 }, negate: false, type: 'in', values: [1, 2], null: true })
    expect(named.categories).toEqual(['car', 'walk'])
    expect(resolveCond(b, named)).toEqual({ col: { column: 0 }, negate: false, type: 'in', values: [1, 3], null: true })
    // Older configs without labels keep their indices.
    const { categories, ...old } = named
    void categories
    expect(resolveCond(b, old)).toMatchObject({ values: [1, 2] })
    expect(() => resolveCond(cat(['car']), named)).toThrow(/walk/)
    // Custom order: walk, bike, car on `a`.
    const sort = nameSort(a, { col: { column: 0 }, desc: false, order: [1, 2, 0] })
    expect(sort.orderCategories).toEqual(['walk', 'bike', 'car'])
    // On `b`: bus and train are not in the saved order and go last.
    expect(resolveSort(b, sort).order).toEqual([3, 2, 5, 0])
  })
})

describe('parquet', () => {
  it('writes a well-formed file', async () => {
    const blob = writeParquet([
      { name: 'person_id', kind: 'utf8', values: ['a', 'bb', 'ccc'] },
      { name: 'row', kind: 'int32', values: [0, 1, 2] },
    ])
    const b = new Uint8Array(await blob.arrayBuffer())
    const text = new TextDecoder().decode(b)
    expect(text.slice(0, 4)).toBe('PAR1')
    expect(text.slice(-4)).toBe('PAR1')
    const metaLen = new DataView(b.buffer).getUint32(b.length - 8, true)
    expect(metaLen).toBeGreaterThan(20)
    expect(metaLen).toBeLessThan(b.length - 12)
    expect(text).toContain('person_id')
  })
})
