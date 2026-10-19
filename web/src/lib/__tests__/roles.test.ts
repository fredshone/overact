import { describe, expect, it } from 'vitest'
import { fileKind, guessAttrId, guessRoles, looksLikeSchedules, mappingProblems } from '../roles'
import type { Inspection } from '../engine/types'

describe('guessRoles', () => {
  it('finds the usual names', () => {
    expect(guessRoles(['pid', 'seq', 'act', 'zone', 'start', 'end'])).toEqual({
      id: 'pid', act: 'act', start: 'start', end: 'end', duration: null, seq: 'seq',
    })
  })
  it('is case-insensitive and uses duration when there is no end', () => {
    const r = guessRoles(['Person ID', 'Activity', 'Start Time', 'Duration'])
    expect(r).toMatchObject({ id: 'Person ID', act: 'Activity', start: 'Start Time', end: null, duration: 'Duration' })
  })
  it('leaves unknown roles empty and never reuses a column', () => {
    const r = guessRoles(['id', 'x', 'y'])
    expect(r).toMatchObject({ id: 'id', act: null, start: null })
  })
  it('tells schedules from attributes', () => {
    expect(looksLikeSchedules(['pid', 'act', 'start', 'end'])).toBe(true)
    expect(looksLikeSchedules(['pid', 'age', 'sex'])).toBe(false)
  })
})

describe('guessAttrId', () => {
  it('prefers the schedules id name', () => {
    expect(guessAttrId(['hid', 'pid', 'age'], 'pid')).toBe('pid')
    expect(guessAttrId(['person_id', 'age'], 'pid')).toBe('person_id')
    expect(guessAttrId(['a', 'b'], 'pid')).toBe('a')
  })
})

describe('mappingProblems', () => {
  const sched = { format: 'csv', columns: [], preview: [], n_rows: null, bytes: 0 } as Inspection
  it('lists what is missing', () => {
    expect(mappingProblems({}, null)).toEqual(['Choose a schedules table.'])
    expect(mappingProblems({ id_col: 'pid', act_col: 'act', start_col: 's', axis: [0, 1440] }, sched)).toEqual([
      'Choose the end (or duration) column.',
    ])
    expect(mappingProblems({ id_col: 'p', act_col: 'a', start_col: 's', duration_col: 'd', axis: [0, 3000] }, sched)).toHaveLength(1)
  })
})

it('fileKind', () => {
  expect(fileKind('x.overact')).toBe('overact')
  expect(fileKind('X.PARQUET')).toBe('table')
  expect(fileKind('a.tsv')).toBe('table')
  expect(fileKind('a.json')).toBe(null)
})
