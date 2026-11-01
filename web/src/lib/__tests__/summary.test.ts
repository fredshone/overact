import { describe, expect, it } from 'vitest'
import type { Summary } from '../engine/types'
import { hexToInt, mix } from '../palette'
import { densityValues, durationTicks, shareTicks, timeTicks, meanMinutes, participation, remapSummary, sharedMax, transitionShares } from '../summary'

// Two activities, two time bins of 10 minutes; two persons: [0] and [0, 1].
function small(): Summary {
  return {
    n_persons: 2,
    n_activities: 2,
    bin: 10,
    bins: 2,
    sequences: [
      { codes: [0], count: 1 },
      { codes: [0, 1], count: 1 },
    ],
    n_distinct: 2,
    // [code][start][duration]
    start_dur: [0, 1, 1, 0, 0, 0, 0, 1].map((x, i) => (i === 1 ? 2 : x)),
    episodes: [0, 2, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0],
    minutes: [1, 1, 1, 0],
    n_episodes: [0, 1, 1],
    trans: [0, 1, 1, 0, 0, 1, 2, 0, 0],
    ngrams: [],
    // One row, two columns: [code 0: 3, code 1: 1] and [code 0: 0, code 1: 4].
    tail: [3, 1, 0, 4],
    tail_rows: 1,
    tail_cols: 2,
  }
}

describe('summary helpers', () => {
  it('moves codes to the union, start/end state last', () => {
    const s = remapSummary(small(), [2, 0], 3)
    expect(s.n_activities).toBe(3)
    expect(s.sequences[1].codes).toEqual([2, 0])
    expect(s.start_dur.slice(8, 12)).toEqual([0, 2, 1, 0])
    expect(s.start_dur.slice(0, 4)).toEqual([0, 0, 0, 1])
    expect(s.episodes.slice(0, 7)).toEqual([1, 1, 0, 0, 0, 0, 0])
    expect(s.episodes.slice(14, 21)).toEqual([0, 2, 0, 0, 0, 0, 0])
    // Old state 2 (day start/end) is new state 3; old 0 -> 2, old 1 -> 0.
    expect(s.trans[3 * 4 + 2]).toBe(2)
    expect(s.trans[2 * 4 + 0]).toBe(1)
    expect(s.trans[0 * 4 + 3]).toBe(1)
    expect(s.tail).toEqual([1, 0, 3, 4, 0, 0])
    expect(remapSummary(small(), [0, 1], 2)).toEqual(small())
  })

  it('computes shares and scales', () => {
    const s = small()
    expect(participation(s, 0)).toBe(1)
    expect(participation(s, 1)).toBe(0.5)
    expect(meanMinutes(s, 0)).toBe(10)
    const d = densityValues(s, 0, 'density')
    expect(Array.from(d)).toEqual([0, 2 / 3, 1 / 3, 0])
    expect(Array.from(densityValues(s, 0, 'starts'))).toEqual([2 / 3, 1 / 3])
    expect(Array.from(densityValues(s, 0, 'durations'))).toEqual([1 / 3, 2 / 3])
    expect(sharedMax([s, s], 2, 'density')).toEqual([2 / 3, 1])
    const t = transitionShares(s)
    expect(t[2 * 3 + 0]).toBe(1)
    expect(t[0 * 3 + 1] + t[0 * 3 + 2]).toBe(1)
  })
})

describe('colours', () => {
  it('reads short and long hex, and mixes', () => {
    expect(hexToInt('#fff')).toBe(0xffffff)
    expect(hexToInt(' #1d1d1f')).toBe(0x1d1d1f)
    expect(mix('#fff', '#000000', 0.5)).toBe('#808080')
  })
})

describe('ticks', () => {
  it('labels hours of the day, durations and shares', () => {
    expect(timeTicks(0, 1440).map((t) => t.label)).toEqual(['00', '06', '12', '18', '24'])
    expect(timeTicks(240, 1440).map((t) => t.label)).toEqual(['06', '12', '18', '24'])
    expect(timeTicks(240, 1440)[0].f).toBeCloseTo(120 / 1440)
    expect(durationTicks(1440).map((t) => t.label)).toEqual(['0', '6h', '12h', '18h', '24h'])
    expect(durationTicks(120).map((t) => t.label)).toEqual(['0', '30m', '1h', '1h30', '2h'])
    expect(shareTicks(0.27).map((t) => t.label)).toEqual(['0%', '10%', '20%'])
    expect(shareTicks(0)).toEqual([])
  })
})
