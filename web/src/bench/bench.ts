// Browser benchmark. Run with `npm run bench -- file.overact` (see scripts/bench.mjs).
//
// Part A times the app path: the real data worker, including messaging,
// column loading and incremental job stepping.
// Part B times the hot loops on the main thread, WASM against plain
// TypeScript on the same data, to check that WASM is worth it.

import init, { WasmEngine } from '../wasm/pkg/overact_wasm.js'
import { EngineClient } from '../lib/engine/client'
import type { Header, ViewSpec } from '../lib/engine/types'
import { filterIn, profile, radixSort, raster } from './baseline'

const params = new URLSearchParams(location.search)
const file = params.get('file') ?? '/demo/demo.overact'
const useThreads = params.get('threads') !== '0'
const skipB = params.get('partB') === '0'
const status = document.getElementById('status')!
const tbody = document.querySelector('#out tbody')!
const results: Record<string, unknown>[] = []

function row(op: string, wasm: number | null, ts: number | null, target: string, notes = '') {
  const tr = document.createElement('tr')
  const f = (v: number | null) => (v === null ? '' : `${v.toFixed(0)} ms`)
  tr.innerHTML = `<td>${op}</td><td class="n">${f(wasm)}</td><td class="n">${f(ts)}</td><td>${target}</td><td>${notes}</td>`
  tbody.appendChild(tr)
  results.push({ op, wasm, ts, target, notes })
}

const say = (s: string) => (status.textContent = s)
const median = (xs: number[]) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)]

async function getBytes(off: number, len: number): Promise<ArrayBuffer> {
  const r = await fetch(file, { headers: { Range: `bytes=${off}-${off + len - 1}` } })
  return r.arrayBuffer()
}

async function partA(): Promise<Header> {
  const client = new EngineClient()
  const ready = new Map<number, (ms: number) => void>()
  const profiled = new Map<number, () => void>()
  const drawn: number[] = []
  const rasterOnly: number[] = []
  client.on((ev) => {
    if (ev.type === 'viewReady') ready.get(ev.view)?.(ev.ms)
    if (ev.type === 'profileReady') profiled.get(ev.view)?.()
    if (ev.type === 'drawn' && !ev.exact) {
      drawn.push(ev.ms)
      rasterOnly.push(ev.rasterMs ?? NaN)
    }
  })
  async function run(view: number, spec: ViewSpec) {
    const t = performance.now()
    const rows = new Promise<number>((r) => ready.set(view, r))
    const prof = new Promise<void>((r) => profiled.set(view, r))
    await client.submit(view, spec)
    const submitted = performance.now() - t
    await rows
    const rowsMs = performance.now() - t
    await Promise.race([prof, new Promise((r) => setTimeout(r, 30_000))])
    return { submitted, rows: rowsMs, all: performance.now() - t }
  }

  say(`opening ${file}`)
  const t0 = performance.now()
  const opened = await client.open({ kind: 'url', url: file }, { fraction: 1, seed: 1, threads: useThreads })
  const h = opened.header
  row(`engine: ${opened.threads} thread${opened.threads > 1 ? 's (+SIMD)' : ''}`, null, null, '', '')
  row(
    `load schedules (${(h.n_persons / 1e6).toFixed(1)}M persons, ${(h.n_segments / 1e6).toFixed(1)}M segments)`,
    performance.now() - t0,
    null,
    'under 10 s at 10M',
    'app path (worker)',
  )
  const col = (name: string) => h.columns.findIndex((c) => c.name === name)
  const emp = col('employment')
  const age = col('age')
  const income = col('income')
  const cats = h.columns[emp].categories!
  const employed = [cats.indexOf('ft-employed'), cats.indexOf('pt-employed')]
  const empCond = { col: { column: emp }, type: 'in' as const, values: employed, negate: false }

  say('filters (app path)')
  const a = await run(1, { filter: [empCond], sort: [] })
  row('first use of a column (load + filter)', a.rows, null, 'under 1 s', `column load ${a.submitted.toFixed(0)} ms; profile done at ${a.all.toFixed(0)} ms`)
  const b = await run(1, { filter: [{ ...empCond, values: [employed[0]] }], sort: [] })
  row('filter, 1 condition (rows ready)', b.rows, null, 'preview < 300 ms, complete < 1 s', `profile done at ${b.all.toFixed(0)} ms`)
  await run(2, { filter: [{ col: { column: age }, type: 'range', min: 0, max: 200, negate: false }], sort: [] })
  const c = await run(2, {
    filter: [empCond, { col: { column: age }, type: 'range', min: 30, max: 60, negate: false }],
    sort: [],
  })
  row('filter, 2 conditions (rows ready)', c.rows, null, 'preview < 300 ms, complete < 1 s', `profile done at ${c.all.toFixed(0)} ms`)
  await run(3, { filter: [{ col: { column: income }, type: 'is_null', negate: true }], sort: [] })
  await run(4, { filter: [empCond], sort: [] })
  const s = await run(4, { filter: [empCond], sort: [{ col: { column: age }, desc: false }, { col: { column: income }, desc: false }] })
  row('2-key sort (age, income) of the employed', s.rows, null, 'under 1.5 s', 'app path; filter reused')

  say('raster (app path)')
  const canvas = document.createElement('canvas').transferControlToOffscreen()
  await client.attachCanvas(4, canvas)
  const cnt = Math.round(h.n_persons * 0.47)
  for (let i = 0; i < 12; i++) {
    await client.draw(4, { width: 1200, height: 800, rowStart: (i * cnt) / 50, rowEnd: cnt, t0: 0, t1: 1440, maxSamples: 32, mode: 0 })
    await new Promise((r) => setTimeout(r, 40))
  }
  await new Promise((r) => setTimeout(r, 300))
  row('raster 1200×800, K=32, incl. canvas upload (median)', median(drawn), null, 'under 30 ms', `raster alone ${median(rasterOnly).toFixed(0)} ms`)
  drawn.length = 0
  rasterOnly.length = 0
  for (let i = 0; i < 12; i++) {
    await client.draw(4, { width: 1200, height: 800, rowStart: (i * cnt) / 50, rowEnd: cnt, t0: 0, t1: 1440, maxSamples: 8, mode: 0 })
    await new Promise((r) => setTimeout(r, 40))
  }
  await new Promise((r) => setTimeout(r, 300))
  row('raster 1200×800, K=8 (while moving), median', median(drawn), null, 'under 30 ms', `raster alone ${median(rasterOnly).toFixed(0)} ms`)
  drawn.length = 0
  rasterOnly.length = 0
  for (let i = 0; i < 12; i++) {
    await client.draw(4, { width: 600, height: 800, rowStart: (i * cnt) / 50, rowEnd: cnt, t0: 0, t1: 1440, maxSamples: 8, mode: 0 })
    await new Promise((r) => setTimeout(r, 40))
  }
  await new Promise((r) => setTimeout(r, 300))
  row('raster 600×800, K=8 (while moving, single-threaded viewer), median', median(drawn), null, 'under 30 ms', `raster alone ${median(rasterOnly).toFixed(0)} ms`)
  const stats = await client.stats()
  row('WASM memory after 4 views', null, null, 'under 1.5 GB', `${(stats.memory / 1e6).toFixed(0)} MB`)
  client.terminate()
  return h
}

async function partB(h: Header) {
  say('part B: loading data on the main thread')
  const out = await init()
  const mem = out.memory
  const e = new WasmEngine(JSON.stringify(h))
  const copy = async (kind: number, column: number) => {
    const ptr = e.part_ptr(kind, column)
    const len = e.part_len(kind, column)
    const off = e.part_offset(kind, column)
    for (let pos = 0; pos < len; pos += 32 << 20) {
      const n = Math.min(32 << 20, len - pos)
      const buf = await getBytes(off + pos, n)
      new Uint8Array(mem.buffer, ptr + pos, n).set(new Uint8Array(buf))
    }
  }
  await copy(0, 0)
  await copy(1, 0)
  e.finish_schedules()
  const col = (name: string) => h.columns.findIndex((c) => c.name === name)
  const emp = col('employment')
  const age = col('age')
  const income = col('income')
  for (const c of [emp, age, income]) {
    await copy(4, c)
    e.finish_column(c)
  }
  const cats = h.columns[emp].categories!
  const employed = [cats.indexOf('ft-employed'), cats.indexOf('pt-employed')]
  const empCond = { col: { column: emp }, type: 'in', values: employed, negate: false }
  // Steps in chunks so rows and profile completion can be timed separately.
  const seen = { finished: new Set<number>(), profiled: new Set<number>() }
  const stepUntil = (key: 'finished' | 'profiled', view: number) => {
    while (!seen[key].has(view)) {
      if (!e.has_work()) throw new Error(`no work left waiting for ${key}`)
      const r = JSON.parse(e.step(1 << 22))
      for (const v of r.finished) seen.finished.add(v)
      for (const v of r.profiled) seen.profiled.add(v)
    }
    seen[key].delete(view)
  }
  const time = (f: () => void) => {
    const t = performance.now()
    f()
    return performance.now() - t
  }

  // WASM, synchronous (one big step).
  e.submit(1, JSON.stringify({ filter: [empCond], sort: [] }))
  const wFilter = time(() => stepUntil('finished', 1))
  const wProfile = time(() => stepUntil('profiled', 1))
  e.submit(1, JSON.stringify({ filter: [empCond], sort: [{ col: { column: age } }, { col: { column: income } }] }))
  const wSort = time(() => stepUntil('finished', 1))
  const count = JSON.parse(e.view_info(1)).count as number
  e.set_palette(new Uint32Array(Array.from({ length: 32 }, (_, i) => (i * 0x3a5f17) & 0xffffff)), 0xffffff)
  const wRaster: number[] = []
  for (let i = 0; i < 9; i++) {
    wRaster.push(time(() => e.raster(1, 1200, 800, (i * count) / 50, count, 0, 1440, 32, 0, 0, 800)))
  }

  // Plain TypeScript on the same data.
  say('part B: TypeScript baseline')
  const offsets = new Uint32Array(await getBytes(h.blobs.offsets.byteOffset, h.blobs.offsets.byteLength))
  const segments = new Uint16Array(await getBytes(h.blobs.segments.byteOffset, h.blobs.segments.byteLength))
  const empCol = new Uint8Array(await getBytes(h.columns[emp].byteOffset, h.columns[emp].byteLength))
  const ageCol = new Int8Array(await getBytes(h.columns[age].byteOffset, h.columns[age].byteLength))
  const incCol = new Float32Array(await getBytes(h.columns[income].byteOffset, h.columns[income].byteLength))
  const set: boolean[] = []
  for (const x of employed) set[x] = true
  let rows: Uint32Array = new Uint32Array()
  const tFilter = time(() => (rows = filterIn(empCol, set)))
  const L = h.time_axis.end - h.time_axis.start
  const tProfile = time(() => profile(offsets, segments, L, h.activities.length, rows))
  let sorted: Uint32Array = rows
  const tSort = time(() => (sorted = radixSort(rows, ageCol, incCol)))
  const colors = new Float32Array(96).map(() => Math.random())
  const px = new Uint8ClampedArray(1200 * 800 * 4)
  const tRaster: number[] = []
  for (let i = 0; i < 5; i++) {
    tRaster.push(time(() => raster(offsets, segments, L, sorted.subarray((i * count) / 50), 1200, 800, 32, colors, px)))
  }

  row(`hot loop: filter ${(h.n_persons / 1e6).toFixed(1)}M rows, 1 condition`, wFilter, tFilter, '', 'same thread, no job stepping')
  row(`hot loop: profile of ${(count / 1e6).toFixed(2)}M persons`, wProfile, tProfile, '', '')
  row(`hot loop: 2-key radix sort of ${(count / 1e6).toFixed(2)}M rows`, wSort, tSort, 'under 1.5 s', '')
  row('hot loop: raster 1200×800, K=32 (median)', median(wRaster), median(tRaster), 'under 30 ms', 'TS skips the sRGB LUT')
  e.free()
}

async function main() {
  const h = await partA()
  if (!skipB) await partB(h)
  say('done')
  console.log('BENCH ' + JSON.stringify(results))
}

main().catch((e) => {
  say(`error: ${e}`)
  console.log('BENCH-ERROR ' + (e instanceof Error ? e.stack : e))
})
