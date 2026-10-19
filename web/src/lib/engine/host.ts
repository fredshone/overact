// The engine host: owns the WASM engine and the dataset in WASM memory, and
// answers the requests of `client.ts`. Long jobs run in small steps; draws
// are handled between steps so the view stays responsive while a filter or
// sort runs. `worker.ts` runs it in a dedicated worker; tests run it directly.

import type { WasmEngine as WasmEngineT } from '../../wasm/pkg/overact_wasm.js'
import type { ColRef, DrawParams, Header, LoadOptions, Request, Source, TableFiles, TableMapping, WorkerEvent } from './types'
import { Gather, makeSubset } from './subset'
import { writeParquet } from '../parquet'

export type Pkg = typeof import('../../wasm/pkg/overact_wasm.js')

/** A loaded WASM build. */
export interface Wasm {
  WasmEngine: Pkg['WasmEngine']
  inspectTable: Pkg['inspect_table']
  memory: WebAssembly.Memory
  threads: number
}

export interface OpenInfo {
  header: Header
  fileHeader: Header
  loadMs: number
  memory: number
  threads: number
  /** Total minutes in each activity over all persons, by code. */
  activityMinutes: number[]
}

interface ByteSource {
  size: number
  read(offset: number, length: number): Promise<ArrayBuffer>
}

const CHUNK = 16 << 20
const PART = { offsets: 0, segments: 1, idOffsets: 2, idData: 3, column: 4 } as const
const MAGIC = new TextEncoder().encode('OVERACT1')

function fileSource(file: Blob): ByteSource {
  return { size: file.size, read: (o, n) => file.slice(o, o + n).arrayBuffer() }
}

async function urlSource(url: string): Promise<ByteSource> {
  // Probe for Range support; fall back to downloading the whole file.
  const probe = await fetch(url, { headers: { Range: 'bytes=0-15' } })
  if (!probe.ok) throw new Error(`cannot fetch ${url}: ${probe.status}`)
  if (probe.status === 206) {
    const total = Number((probe.headers.get('Content-Range') ?? '').split('/')[1] ?? NaN)
    await probe.arrayBuffer()
    return {
      size: total,
      async read(o, n) {
        const r = await fetch(url, { headers: { Range: `bytes=${o}-${o + n - 1}` } })
        if (r.status !== 206) throw new Error(`range request failed: ${r.status}`)
        return r.arrayBuffer()
      },
    }
  }
  const blob = await probe.blob()
  return fileSource(blob)
}

async function readHeader(src: ByteSource): Promise<Header> {
  const pre = new DataView(await src.read(0, 12))
  const magic = new TextDecoder().decode(new Uint8Array(pre.buffer, 0, 8))
  if (magic !== 'OVERACT1') throw new Error('not an .overact file')
  const len = pre.getUint32(8, true)
  return JSON.parse(new TextDecoder().decode(await src.read(12, len)))
}

// Tables are read synchronously from inside WASM (ingest is one long call).
let syncReader: FileReaderSync | null = null
function readSync(blob: Blob, offset: number, length: number): Uint8Array {
  syncReader ??= new FileReaderSync()
  return new Uint8Array(syncReader.readAsArrayBuffer(blob.slice(offset, offset + length)))
}

function yieldToEvents(): Promise<void> {
  return new Promise((resolve) => {
    const ch = new MessageChannel()
    ch.port1.onmessage = () => {
      ch.port1.close()
      resolve()
    }
    ch.port2.postMessage(null)
  })
}

export class Host {
  private engine: WasmEngineT | null = null
  private header: Header | null = null // header as seen by the engine (subset counts)
  private source: ByteSource | null = null
  private gather: Gather | null = null
  private canvases = new Map<number, OffscreenCanvasRenderingContext2D>()
  private pendingDraws = new Map<number, { params: DrawParams; exact: boolean }>()
  /** Exact (unsampled) refinements, drawn in bands so the worker stays responsive. */
  private exactJobs = new Map<number, { params: DrawParams; y: number; rows: number; t: number }>()
  private loadingColumns = new Map<number, Promise<void>>()
  private budget = 1 << 20
  private submitted = new Map<number, number>()
  /** Latest submit request per view: an older one still loading columns must not overwrite it. */
  private latestSubmit = new Map<number, number>()
  private submitSeq = 0
  /** Bumped when the dataset is replaced; loads for an older one stop writing. */
  private generation = 0
  private idsLoading: Promise<void> | null = null
  private looping = false

  constructor(
    private wasm: Wasm,
    private post: (ev: WorkerEvent) => void,
  ) {}

  private get memory() {
    return this.wasm.memory
  }

  /** Throws if the dataset was replaced since `gen` (its buffers may be freed). */
  private checkGeneration(gen: number) {
    if (gen !== this.generation) throw new Error('dataset closed')
  }

  /** Copies `length` bytes from the source at `offset` into WASM memory at `ptr`. */
  private async copyInto(ptr: number, offset: number, length: number, phase: string, gen: number) {
    const src = this.source!
    for (let pos = 0; pos < length; pos += CHUNK) {
      const n = Math.min(CHUNK, length - pos)
      const buf = await src.read(offset + pos, n)
      this.checkGeneration(gen)
      if (buf.byteLength !== n) throw new Error('file is truncated')
      new Uint8Array(this.memory.buffer, ptr + pos, n).set(new Uint8Array(buf))
      this.post({ type: 'progress', phase, done: pos + n, total: length })
    }
  }

  private async loadPart(kind: number, column: number, phase: string) {
    const e = this.engine!
    const gen = this.generation
    const ptr = e.part_ptr(kind, column)
    const len = e.part_len(kind, column)
    if (this.gather) {
      const memoryNow = () => {
        this.checkGeneration(gen)
        return this.memory
      }
      await this.gather.fill(kind, column, ptr, len, memoryNow, (done, total) =>
        this.post({ type: 'progress', phase, done, total }),
      )
    } else {
      await this.copyInto(ptr, e.part_offset(kind, column), len, phase, gen)
    }
  }

  private reset() {
    this.generation++
    this.engine?.free()
    this.engine = null
    this.canvases.clear()
    this.pendingDraws.clear()
    this.exactJobs.clear()
    this.loadingColumns.clear()
    this.latestSubmit.clear()
    this.submitted.clear()
    this.idsLoading = null
  }

  private info(fileHeader: Header, t0: number): OpenInfo {
    const e = this.engine!
    return {
      header: this.header!,
      fileHeader,
      loadMs: performance.now() - t0,
      memory: e.memory_bytes(),
      threads: this.wasm.threads,
      activityMinutes: Array.from(e.activity_minutes()),
    }
  }

  private async open(src: Source, options: LoadOptions): Promise<OpenInfo> {
    this.reset()
    const source = src.kind === 'file' ? fileSource(src.file) : await urlSource(src.url)
    this.source = source
    const fileHeader = await readHeader(source)
    this.header = fileHeader
    this.gather = null
    if (options.fraction < 1) {
      const s = await makeSubset(fileHeader, source, options, (d, t) =>
        this.post({ type: 'progress', phase: 'choosing subset', done: d, total: t }),
      )
      this.header = s.header
      this.gather = s.gather
    }
    const t0 = performance.now()
    const e = new this.wasm.WasmEngine(JSON.stringify(this.header))
    this.engine = e
    await this.loadPart(PART.offsets, 0, 'offsets')
    await this.loadPart(PART.segments, 0, 'schedules')
    e.finish_schedules()
    const info = this.info(fileHeader, t0)
    // Person ids are only needed for hover and export; load them in the background.
    if (this.header.blobs.person_id_offsets) {
      const gen = this.generation
      this.idsLoading = this.loadPart(PART.idOffsets, 0, 'ids').then(() => this.loadPart(PART.idData, 0, 'ids'))
      this.idsLoading.catch((err) => {
        if (gen === this.generation) this.post({ type: 'error', message: `loading person ids: ${err}` })
      })
    }
    return info
  }

  private openTables(tables: TableFiles, mapping: TableMapping, options: LoadOptions): OpenInfo {
    this.reset()
    this.source = null
    this.gather = null
    const t0 = performance.now()
    const { schedules: s, attributes: a } = tables
    const config = {
      ...mapping,
      fraction: options.fraction,
      seed: options.seed,
      activities_name: s.name,
      attributes_name: a?.name ?? null,
    }
    this.engine = this.wasm.WasmEngine.from_tables(
      s, s.size, s.name,
      a ?? null, a?.size ?? 0, a?.name ?? '',
      readSync,
      JSON.stringify(config),
      (phase: string, done: number, total: number) => this.post({ type: 'progress', phase, done, total }),
    )
    this.header = JSON.parse(this.engine.header_json()) as Header
    return this.info(this.header, t0)
  }

  /** The dataset in memory written as an `.overact` file (loads any lazy columns first). */
  private async saveOveract(): Promise<Blob> {
    const e = this.engine!
    for (let i = 0; i < this.header!.columns.length; i++) await this.ensureColumn(i)
    await this.idsLoading
    const json = e.file_header()
    const h = JSON.parse(json) as Header
    const hb = new TextEncoder().encode(json)
    const len = new Uint8Array(4)
    new DataView(len.buffer).setUint32(0, hb.length, true)
    const parts: BlobPart[] = [MAGIC, len, hb]
    let pos = 12 + hb.length
    const blobs: [number, number, { byteOffset: number }][] = [
      [PART.offsets, 0, h.blobs.offsets],
      [PART.segments, 0, h.blobs.segments],
    ]
    if (h.blobs.person_id_offsets && h.blobs.person_id_data) {
      blobs.push([PART.idOffsets, 0, h.blobs.person_id_offsets], [PART.idData, 0, h.blobs.person_id_data])
    }
    h.columns.forEach((c, i) => blobs.push([PART.column, i, c]))
    for (const [kind, col, b] of blobs) {
      parts.push(new Uint8Array(b.byteOffset - pos))
      const ptr = e.part_view_ptr(kind, col)
      const n = e.part_view_len(kind, col)
      if (!ptr && n) throw new Error('part not loaded')
      parts.push(new Uint8Array(this.memory.buffer, ptr, n).slice())
      pos = b.byteOffset + n
    }
    parts.push(new Uint8Array(((pos + 7) & ~7) - pos))
    return new Blob(parts, { type: 'application/octet-stream' })
  }

  private ensureColumn(i: number): Promise<void> {
    // A load in progress first: column_loaded is false until it finishes.
    let p = this.loadingColumns.get(i)
    if (!p && this.engine!.column_loaded(i)) return Promise.resolve()
    if (!p) {
      const gen = this.generation
      p = (async () => {
        await this.loadPart(PART.column, i, `column ${this.header!.columns[i].name}`)
        this.checkGeneration(gen)
        this.engine!.finish_column(i)
      })().finally(() => {
        if (gen === this.generation) this.loadingColumns.delete(i)
      })
      this.loadingColumns.set(i, p)
    }
    return p
  }

  private async ensureColumns(cols: ColRef[]) {
    for (const c of cols) if ('column' in c) await this.ensureColumn(c.column)
  }

  /** Pixels in WASM memory as an ImageData-compatible array (copied if memory is shared). */
  private pixels(ptr: number, len: number): Uint8ClampedArray<ArrayBuffer> {
    const view = new Uint8ClampedArray(this.memory.buffer, ptr, len)
    return (this.memory.buffer instanceof ArrayBuffer ? view : view.slice()) as Uint8ClampedArray<ArrayBuffer>
  }

  private rasterRows(view: number, p: DrawParams, maxSamples: number, y0: number, y1: number): number {
    return this.engine!.raster(view, p.width, p.height, p.rowStart, p.rowEnd, p.t0, p.t1, maxSamples, p.mode, y0, y1)
  }

  /** A full draw with sampling. */
  private draw(view: number, params: DrawParams) {
    const ctx = this.canvases.get(view)
    if (!ctx || !this.engine) return
    const t = performance.now()
    const { width, height } = params
    if (width <= 0 || height <= 0) return
    const ptr = this.rasterRows(view, params, params.maxSamples, 0, height)
    if (!ptr) return
    const rasterMs = performance.now() - t
    sizeCanvas(ctx, width, height)
    ctx.putImageData(new ImageData(this.pixels(ptr, width * height * 4), width, height), 0, 0)
    this.post({ type: 'drawn', view, ms: performance.now() - t, rasterMs, exact: false })
  }

  private exactStep(view: number) {
    const job = this.exactJobs.get(view)!
    const ctx = this.canvases.get(view)
    const p = job.params
    if (!ctx || !this.engine) {
      this.exactJobs.delete(view)
      return
    }
    const t = performance.now()
    const y1 = Math.min(p.height, job.y + job.rows)
    const ptr = this.rasterRows(view, p, 0, job.y, y1)
    if (!ptr) {
      this.exactJobs.delete(view)
      return
    }
    sizeCanvas(ctx, p.width, p.height)
    ctx.putImageData(new ImageData(this.pixels(ptr, p.width * (y1 - job.y) * 4), p.width, y1 - job.y), 0, job.y)
    const dt = performance.now() - t
    job.rows = Math.max(1, Math.round((job.rows * 12) / Math.max(dt, 0.5)))
    job.y = y1
    if (y1 >= p.height) {
      this.exactJobs.delete(view)
      this.post({ type: 'drawn', view, ms: performance.now() - job.t, exact: true })
    }
  }

  private kick() {
    if (this.looping) return
    this.looping = true
    this.loop().finally(() => (this.looping = false))
  }

  private async loop() {
    for (;;) {
      for (const [view, d] of this.pendingDraws) {
        this.pendingDraws.delete(view)
        if (d.exact) {
          this.exactJobs.set(view, { params: d.params, y: 0, rows: 16, t: performance.now() })
        } else {
          this.exactJobs.delete(view)
          this.draw(view, d.params)
        }
      }
      const e = this.engine
      if (e?.has_work()) {
        const t = performance.now()
        const res = JSON.parse(e.step(this.budget)) as {
          finished: number[]
          profiled: number[]
          pending: number[]
          progress: number
        }
        const dt = performance.now() - t
        // Aim for ~12 ms per step.
        this.budget = Math.max(1 << 14, Math.min(1 << 24, Math.round((this.budget * 12) / Math.max(dt, 1))))
        for (const v of res.finished) {
          const ms = performance.now() - (this.submitted.get(v) ?? performance.now())
          this.post({ type: 'viewReady', view: v, info: JSON.parse(e.view_info(v)), ms })
        }
        for (const v of res.profiled) this.post({ type: 'profileReady', view: v })
        this.post({ type: 'jobProgress', pending: res.pending, progress: res.progress })
        await yieldToEvents()
        continue
      }
      if (this.exactJobs.size) {
        this.exactStep(this.exactJobs.keys().next().value!)
        await yieldToEvents()
        continue
      }
      if (this.pendingDraws.size === 0) return
    }
  }

  private values(cols: ColRef[], p: number): (number | null)[] {
    return cols.map((c) => {
      const v = this.engine!.value(JSON.stringify(c), p)
      return Number.isNaN(v) ? null : v
    })
  }

  /** The window's persons in display order: id (or index) and row position. */
  private exportIds(view: number, format: 'csv' | 'parquet'): Blob {
    const e = this.engine!
    const info = JSON.parse(e.view_info(view))
    if (!info) throw new Error('view not ready')
    const bytes = e.export_ids(view, format === 'csv')
    if (format === 'csv') return new Blob([bytes as Uint8Array<ArrayBuffer>], { type: 'text/csv' })
    const n = info.count as number
    const offsets = new Uint32Array(bytes.buffer, bytes.byteOffset, n + 1)
    const data = bytes.subarray((n + 1) * 4)
    return writeParquet([
      { name: 'person_id', kind: 'utf8', offsets, data },
      { name: 'row', kind: 'int32', values: Int32Array.from({ length: n }, (_, i) => i) },
    ])
  }

  async handle(req: Request): Promise<unknown> {
    switch (req.type) {
      case 'open':
        return this.open(req.source, req.options)
      case 'inspect':
        return JSON.parse(this.wasm.inspectTable(req.file, req.file.size, req.file.name, readSync))
      case 'openTables':
        return this.openTables(req.tables, req.mapping, req.options)
      case 'saveOveract':
        return this.saveOveract()
      case 'palette':
        this.engine?.set_palette(new Uint32Array(req.colors), req.background)
        return null
      case 'submit': {
        const e = this.engine!
        const seq = ++this.submitSeq
        this.latestSubmit.set(req.view, seq)
        await this.ensureColumns([...req.spec.filter.map((c) => c.col), ...req.spec.sort.map((k) => k.col)])
        // Superseded by a newer submit while loading columns, or the view was dropped.
        if (this.latestSubmit.get(req.view) !== seq || e !== this.engine) return null
        this.submitted.set(req.view, performance.now())
        if (e.submit(req.view, JSON.stringify(req.spec))) this.kick()
        // Already showing this spec: no job, so report the view as ready now.
        else this.post({ type: 'viewReady', view: req.view, info: JSON.parse(e.view_info(req.view)), ms: 0 })
        return null
      }
      case 'dropView':
        this.latestSubmit.delete(req.view)
        this.engine?.drop_view(req.view)
        this.canvases.delete(req.view)
        this.pendingDraws.delete(req.view)
        this.exactJobs.delete(req.view)
        return null
      case 'attachCanvas': {
        const ctx = req.canvas.getContext('2d', { alpha: false })
        if (!ctx) throw new Error('no 2d context in worker')
        this.canvases.set(req.view, ctx)
        return null
      }
      case 'draw':
        this.pendingDraws.set(req.view, { params: req.params, exact: req.exact })
        this.kick()
        return null
      case 'profile':
        return this.engine!.profile(req.view) ?? null
      case 'gutter':
        await this.ensureColumns([req.col])
        return this.engine!.gutter(req.view, JSON.stringify(req.col), req.height, req.rowStart, req.rowEnd)
      case 'hover': {
        const e = this.engine!
        const p = e.person_at(req.view, req.pos)
        if (p < 0) return null
        return { person: JSON.parse(e.person_info(p)), values: this.values(req.cols, p) }
      }
      case 'columnRange':
        return JSON.parse(this.engine!.column_range(JSON.stringify(req.col)))
      case 'exportIds':
        return this.exportIds(req.view, req.format)
      case 'snapshot': {
        const ctx = this.canvases.get(req.view)
        if (!ctx || !ctx.canvas.width) return null
        return createImageBitmap(ctx.canvas)
      }
      case 'stats':
        return { memory: this.engine?.memory_bytes() ?? 0, threads: this.wasm.threads }
    }
  }
}

function sizeCanvas(ctx: OffscreenCanvasRenderingContext2D, width: number, height: number) {
  const canvas = ctx.canvas
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width
    canvas.height = height
  }
}
