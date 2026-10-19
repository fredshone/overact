// "Load random subset": keeps a random fraction of persons, streaming the
// file and copying only the kept persons' data into WASM memory.

import type { Header, LoadOptions } from './types'

interface ByteSource {
  read(offset: number, length: number): Promise<ArrayBuffer>
}

type Progress = (done: number, total: number) => void

const WINDOW_BYTES = 16 << 20

function hash(p: number, seed: number): number {
  let h = Math.imul(p ^ seed, 0x9e3779b1)
  h ^= h >>> 16
  h = Math.imul(h, 0x85ebca6b)
  h ^= h >>> 13
  h = Math.imul(h, 0xc2b2ae35)
  h ^= h >>> 16
  return (h >>> 0) / 4294967296
}

async function readU32(src: ByteSource, offset: number, count: number, progress: Progress) {
  const out = new Uint32Array(count)
  const bytes = new Uint8Array(out.buffer)
  for (let pos = 0; pos < count * 4; pos += WINDOW_BYTES) {
    const n = Math.min(WINDOW_BYTES, count * 4 - pos)
    bytes.set(new Uint8Array(await src.read(offset + pos, n)), pos)
    progress(pos + n, count * 4)
  }
  return out
}

/** Re-maps kept persons' CSR rows (offsets + data) from the file into a new buffer. */
function remapOffsets(orig: Uint32Array, kept: Uint32Array): Uint32Array {
  const out = new Uint32Array(kept.length + 1)
  for (let i = 0; i < kept.length; i++) {
    const p = kept[i]
    out[i + 1] = out[i] + (orig[p + 1] - orig[p])
  }
  return out
}

export class Gather {
  constructor(
    private src: ByteSource,
    private file: Header,
    private kept: Uint32Array,
    private segOffsets: { orig: Uint32Array; next: Uint32Array },
    private idOffsets: { orig: Uint32Array; next: Uint32Array } | null,
  ) {}

  /** Fills an engine part (see PART in worker.ts) at `ptr` with kept persons only. */
  async fill(
    kind: number,
    column: number,
    ptr: number,
    len: number,
    memory: () => WebAssembly.Memory,
    progress: Progress,
  ) {
    const b = this.file.blobs
    const write = (bytes: Uint8Array, at: number) => new Uint8Array(memory().buffer, ptr + at, bytes.length).set(bytes)
    if (kind === 0) {
      write(new Uint8Array(this.segOffsets.next.buffer), 0)
    } else if (kind === 2) {
      write(new Uint8Array(this.idOffsets!.next.buffer), 0)
    } else if (kind === 1) {
      await this.gatherRows(b.segments.byteOffset, 2, this.segOffsets, write, progress)
    } else if (kind === 3) {
      await this.gatherRows(b.person_id_data!.byteOffset, 1, this.idOffsets!, write, progress)
    } else {
      const meta = this.file.columns[column]
      const size = meta.byteLength / this.file.n_persons
      await this.gatherFixed(meta.byteOffset, size, write, progress)
    }
    void len
  }

  /** Fixed-size values: one per person. */
  private async gatherFixed(base: number, size: number, write: (b: Uint8Array, at: number) => void, progress: Progress) {
    const kept = this.kept
    const perWindow = Math.max(1, Math.floor(WINDOW_BYTES / size))
    const n = this.file.n_persons
    let k = 0
    for (let p0 = 0; p0 < n && k < kept.length; p0 += perWindow) {
      const p1 = Math.min(n, p0 + perWindow)
      const src = new Uint8Array(await this.src.read(base + p0 * size, (p1 - p0) * size))
      const start = k
      while (k < kept.length && kept[k] < p1) k++
      const out = new Uint8Array((k - start) * size)
      for (let i = start; i < k; i++) {
        const o = (kept[i] - p0) * size
        out.set(src.subarray(o, o + size), (i - start) * size)
      }
      write(out, start * size)
      progress(p1, n)
    }
  }

  /** Variable-length CSR rows (segments or id bytes). */
  private async gatherRows(
    base: number,
    size: number,
    offs: { orig: Uint32Array; next: Uint32Array },
    write: (b: Uint8Array, at: number) => void,
    progress: Progress,
  ) {
    const kept = this.kept
    const { orig, next } = offs
    const n = this.file.n_persons
    let k = 0
    while (k < kept.length) {
      // A window of kept persons whose source rows span at most WINDOW_BYTES.
      const first = kept[k]
      let end = k + 1
      while (end < kept.length && (orig[kept[end] + 1] - orig[first]) * size <= WINDOW_BYTES) end++
      const lo = orig[first]
      const hi = orig[kept[end - 1] + 1]
      const src = new Uint8Array(await this.src.read(base + lo * size, (hi - lo) * size))
      const out = new Uint8Array((next[end] - next[k]) * size)
      for (let i = k; i < end; i++) {
        const p = kept[i]
        const a = (orig[p] - lo) * size
        const b = (orig[p + 1] - lo) * size
        out.set(src.subarray(a, b), (next[i] - next[k]) * size)
      }
      write(out, next[k] * size)
      k = end
      progress(kept[k - 1], n)
    }
  }
}

export async function makeSubset(
  file: Header,
  src: ByteSource,
  options: LoadOptions,
  progress: Progress,
): Promise<{ header: Header; gather: Gather }> {
  const n = file.n_persons
  const keptList: number[] = []
  for (let p = 0; p < n; p++) if (hash(p, options.seed) < options.fraction) keptList.push(p)
  if (keptList.length === 0) keptList.push(0)
  const kept = Uint32Array.from(keptList)
  const k = kept.length

  const origSeg = await readU32(src, file.blobs.offsets.byteOffset, n + 1, progress)
  const nextSeg = remapOffsets(origSeg, kept)
  let ids: { orig: Uint32Array; next: Uint32Array } | null = null
  const header: Header = structuredClone(file)
  header.n_persons = k
  header.n_segments = nextSeg[k]
  header.blobs.offsets.byteLength = (k + 1) * 4
  header.blobs.segments.byteLength = nextSeg[k] * 2
  if (file.blobs.person_id_offsets && file.blobs.person_id_data) {
    const orig = await readU32(src, file.blobs.person_id_offsets.byteOffset, n + 1, progress)
    ids = { orig, next: remapOffsets(orig, kept) }
    header.blobs.person_id_offsets!.byteLength = (k + 1) * 4
    header.blobs.person_id_data!.byteLength = ids.next[k]
  }
  for (const c of header.columns) {
    c.byteLength = (c.byteLength / n) * k
    // Ranges and null counts describe the whole file; keep min/max as display bounds.
    delete c.nullCount
  }
  header.meta = { ...header.meta, subset: { fraction: options.fraction, seed: options.seed, of: n } }
  return { header, gather: new Gather(src, file, kept, { orig: origSeg, next: nextSeg }, ids) }
}
