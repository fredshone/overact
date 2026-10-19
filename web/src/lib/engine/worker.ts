/// <reference lib="webworker" />
// The data worker: loads the WASM build and runs the engine host (host.ts).

import type { Request } from './types'
import { Host, type Pkg, type Wasm } from './host'

declare const self: DedicatedWorkerGlobalScope

type Reply = { id: number; ok: true; result: unknown } | { id: number; ok: false; error: string }

/**
 * Loads the threaded + SIMD build when the page is cross-origin isolated
 * (SharedArrayBuffer available), else the single-threaded build.
 */
async function load(wantThreads: boolean): Promise<Wasm> {
  if (wantThreads && self.crossOriginIsolated && typeof SharedArrayBuffer !== 'undefined') {
    try {
      const mt = (await import('../../wasm/pkg-mt/overact_wasm.js')) as unknown as Pkg & {
        initThreadPool(n: number): Promise<void>
      }
      const out = await mt.default()
      const n = Math.max(1, Math.min(16, navigator.hardwareConcurrency || 4))
      await mt.initThreadPool(n)
      return { WasmEngine: mt.WasmEngine, inspectTable: mt.inspect_table, memory: out.memory, threads: mt.thread_count() }
    } catch (e) {
      console.warn('threaded engine unavailable, using single-threaded', e)
    }
  }
  const st = await import('../../wasm/pkg/overact_wasm.js')
  const out = await st.default()
  return { WasmEngine: st.WasmEngine, inspectTable: st.inspect_table, memory: out.memory, threads: 1 }
}

let host: Promise<Host> | null = null

self.onmessage = async (ev: MessageEvent<{ id: number; req: Request }>) => {
  const { id, req } = ev.data
  try {
    host ??= load(!('options' in req) || req.options.threads !== false).then(
      (wasm) => new Host(wasm, (e) => self.postMessage(e)),
    )
    const result = await (await host).handle(req)
    const transfer: Transferable[] =
      result instanceof Uint32Array ? [result.buffer] : result instanceof ImageBitmap ? [result] : []
    self.postMessage({ id, ok: true, result } satisfies Reply, transfer)
  } catch (e) {
    self.postMessage({ id, ok: false, error: e instanceof Error ? e.message : String(e) } satisfies Reply)
  }
}
