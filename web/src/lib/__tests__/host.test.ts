/// <reference types="node" />
// The worker protocol end to end: the engine host (what the worker runs)
// against the real single-threaded WASM build and the small demo dataset.
// Needs `scripts/build-wasm.sh` first; skipped when the build is missing.

import { existsSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { beforeAll, describe, expect, it } from 'vitest'
import { Host, type OpenInfo, type Pkg } from '../engine/host'
import type { Request, ViewSpec, WorkerEvent } from '../engine/types'

const url = (p: string) => fileURLToPath(new URL(p, import.meta.url))
const WASM = url('../../wasm/pkg/overact_wasm_bg.wasm')
const DEMO = url('../../../public/demo/demo-small.overact')

describe.skipIf(!existsSync(WASM))('engine host', () => {
  let host: Host
  let info: OpenInfo
  const events: WorkerEvent[] = []

  /** Resolves with the first event (from now on) that matches. */
  function next<T extends WorkerEvent['type']>(type: T, view?: number): Promise<Extract<WorkerEvent, { type: T }>> {
    const from = events.length
    return new Promise((resolve) => {
      const poll = () => {
        const ev = events.slice(from).find((e) => e.type === type && (view === undefined || ('view' in e && e.view === view)))
        if (ev) resolve(ev as Extract<WorkerEvent, { type: T }>)
        else setTimeout(poll, 1)
      }
      poll()
    })
  }

  const call = <T>(req: Request) => host.handle(req) as Promise<T>

  beforeAll(async () => {
    const pkg: Pkg = await import('../../wasm/pkg/overact_wasm.js')
    const out = await pkg.default({ module_or_path: readFileSync(WASM) })
    host = new Host({ WasmEngine: pkg.WasmEngine, inspectTable: pkg.inspect_table, memory: out.memory, threads: 1 }, (e) =>
      events.push(e),
    )
    const file = new File([readFileSync(DEMO)], 'demo-small.overact')
    info = await call<OpenInfo>({ type: 'open', source: { kind: 'file', file }, options: { fraction: 1, seed: 1 } })
  })

  it('opens a file', () => {
    expect(info.header.n_persons).toBe(200)
    expect(info.activityMinutes).toHaveLength(info.header.activities.length)
    expect(info.activityMinutes.reduce((a, b) => a + b, 0)).toBe(200 * 1440)
  })

  it('runs a view job and answers queries about it', async () => {
    const employment = info.header.columns.findIndex((c) => c.name === 'employment')
    const spec: ViewSpec = {
      filter: [{ col: { column: employment }, negate: false, type: 'in', values: [0] }],
      sort: [{ col: { feature: 'measure', measure: 'duration', activity: 0, not: false, nth: 0 }, desc: true }],
    }
    const ready = next('viewReady', 1)
    const profiled = next('profileReady', 1)
    await call({ type: 'submit', view: 1, spec })
    const { info: v } = await ready
    expect(v.count).toBeGreaterThan(0)
    expect(v.count).toBeLessThan(200)
    await profiled

    // Every minute's activity counts add up to the window's persons.
    const L = info.header.time_axis.end - info.header.time_axis.start
    const p = await call<Uint32Array>({ type: 'profile', view: 1 })
    expect(p).toHaveLength(info.header.activities.length * L)
    for (const m of [0, 600, L - 1]) {
      let sum = 0
      for (let a = 0; a < info.header.activities.length; a++) sum += p[a * L + m]
      expect(sum).toBe(v.count)
    }

    // Rows are sorted by time at home, longest first.
    const col = spec.sort[0].col
    const first = await call<{ values: (number | null)[] }>({ type: 'hover', view: 1, pos: 0, cols: [col] })
    const last = await call<{ values: (number | null)[] }>({ type: 'hover', view: 1, pos: v.count - 1, cols: [col] })
    expect(first.values[0]!).toBeGreaterThanOrEqual(last.values[0]!)
    expect(await call({ type: 'hover', view: 1, pos: v.count, cols: [] })).toBeNull()

    const range = await call<{ min: number; max: number }>({ type: 'columnRange', col })
    expect(range.max).toBeGreaterThanOrEqual(first.values[0]!)

    const csv = await (await call<Blob>({ type: 'exportIds', view: 1, format: 'csv' })).text()
    const lines = csv.trim().split('\n')
    expect(lines[0]).toBe('person_id,row')
    expect(lines).toHaveLength(v.count + 1)
    expect(lines[1].endsWith(',0')).toBe(true)
    const pq = await call<Blob>({ type: 'exportIds', view: 1, format: 'parquet' })
    expect(new TextDecoder().decode(new Uint8Array(await pq.slice(0, 4).arrayBuffer()))).toBe('PAR1')
  })

  it('reports an unchanged spec as ready at once', async () => {
    const ready = next('viewReady', 2)
    await call({ type: 'submit', view: 2, spec: { filter: [], sort: [] } })
    expect((await ready).info.count).toBe(200)
    const again = next('viewReady', 2)
    await call({ type: 'submit', view: 2, spec: { filter: [], sort: [] } })
    expect((await again).ms).toBe(0)
  })

  it('reports engine errors', async () => {
    await expect(call({ type: 'submit', view: 3, spec: { filter: [], sort: [{ col: { column: 999 }, desc: false }] } })).rejects.toThrow()
  })
})
