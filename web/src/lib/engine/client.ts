// Main-thread handle on the data worker: promise-based requests plus events.

import type {
  ColRef,
  DrawParams,
  Inspection,
  LoadOptions,
  PersonInfo,
  Request,
  Source,
  TableFiles,
  TableMapping,
  ViewSpec,
  WorkerEvent,
} from './types'

type Pending = { resolve: (v: unknown) => void; reject: (e: Error) => void }

import type { OpenInfo } from './host'

export type OpenResult = OpenInfo

export interface Hover {
  person: PersonInfo
  values: (number | null)[]
}

export class EngineClient {
  private worker: Worker
  private nextId = 1
  private pending = new Map<number, Pending>()
  private listeners = new Set<(ev: WorkerEvent) => void>()

  constructor() {
    this.worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' })
    this.worker.onmessage = (ev) => {
      const d = ev.data
      if (typeof d.id === 'number') {
        const p = this.pending.get(d.id)
        this.pending.delete(d.id)
        if (!p) return
        if (d.ok) p.resolve(d.result)
        else p.reject(new Error(d.error))
      } else {
        for (const l of this.listeners) l(d as WorkerEvent)
      }
    }
    this.worker.onerror = (e) => {
      for (const l of this.listeners) l({ type: 'error', message: e.message })
    }
  }

  private call<T>(req: Request, transfer: Transferable[] = []): Promise<T> {
    const id = this.nextId++
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject })
      this.worker.postMessage({ id, req }, transfer)
    })
  }

  on(fn: (ev: WorkerEvent) => void): () => void {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }

  open(source: Source, options: LoadOptions) {
    return this.call<OpenResult>({ type: 'open', source, options })
  }
  /** Column names, types and first rows of a CSV/Parquet file. */
  inspect(file: File) {
    return this.call<Inspection>({ type: 'inspect', file })
  }
  /** Reads, cleans and packs the tables straight into the engine. */
  openTables(tables: TableFiles, mapping: TableMapping, options: LoadOptions) {
    return this.call<OpenResult>({ type: 'openTables', tables, mapping, options })
  }
  /** The loaded dataset as an `.overact` file. */
  saveOveract() {
    return this.call<Blob>({ type: 'saveOveract' })
  }
  palette(colors: number[], background: number) {
    return this.call<null>({ type: 'palette', colors, background })
  }
  submit(view: number, spec: ViewSpec) {
    return this.call<null>({ type: 'submit', view, spec })
  }
  dropView(view: number) {
    return this.call<null>({ type: 'dropView', view })
  }
  attachCanvas(view: number, canvas: OffscreenCanvas) {
    return this.call<null>({ type: 'attachCanvas', view, canvas }, [canvas])
  }
  draw(view: number, params: DrawParams, exact = false) {
    return this.call<null>({ type: 'draw', view, params, exact })
  }
  profile(view: number) {
    return this.call<Uint32Array | null>({ type: 'profile', view })
  }
  gutter(view: number, col: ColRef, height: number, rowStart: number, rowEnd: number) {
    return this.call<Float64Array>({ type: 'gutter', view, col, height, rowStart, rowEnd })
  }
  hover(view: number, pos: number, cols: ColRef[]) {
    return this.call<Hover | null>({ type: 'hover', view, pos, cols })
  }
  columnRange(col: ColRef) {
    return this.call<{ min: number | null; max: number | null } | null>({ type: 'columnRange', col })
  }
  exportIds(view: number, format: 'csv' | 'parquet' = 'csv') {
    return this.call<Blob>({ type: 'exportIds', view, format })
  }
  /** The window's current raster as an ImageBitmap (device pixels). */
  snapshot(view: number) {
    return this.call<ImageBitmap | null>({ type: 'snapshot', view })
  }
  stats() {
    return this.call<{ memory: number; threads: number }>({ type: 'stats' })
  }

  /** Drops the worker (and its WASM memory, which never shrinks). */
  terminate() {
    this.worker.terminate()
    for (const p of this.pending.values()) p.reject(new Error('engine closed'))
    this.pending.clear()
  }
}
