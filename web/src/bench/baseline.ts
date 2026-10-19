// Plain TypeScript versions of the hot loops, to check that WASM is worth it.
// They mirror the Rust algorithms (same data layout, same radix sort).

export function filterIn(col: Uint8Array, set: boolean[]): Uint32Array {
  const out = new Uint32Array(col.length)
  let n = 0
  for (let i = 0; i < col.length; i++) if (set[col[i]]) out[n++] = i
  return out.slice(0, n)
}

export function profile(
  offsets: Uint32Array,
  segments: Uint16Array,
  axisLen: number,
  nAct: number,
  rows: Uint32Array,
): Uint32Array {
  const stride = axisLen + 1
  const diff = new Int32Array(nAct * stride)
  for (let r = 0; r < rows.length; r++) {
    const p = rows[r]
    const a = offsets[p]
    const b = offsets[p + 1]
    for (let s = a; s < b; s++) {
      const seg = segments[s]
      const end = s + 1 < b ? segments[s + 1] >>> 5 : axisLen
      const base = (seg & 31) * stride
      diff[base + (seg >>> 5)]++
      diff[base + end]--
    }
  }
  const out = new Uint32Array(nAct * axisLen)
  for (let c = 0; c < nAct; c++) {
    let run = 0
    for (let t = 0; t < axisLen; t++) {
      run += diff[c * stride + t]
      out[c * axisLen + t] = run
    }
  }
  return out
}

export function refineRange(rows: Uint32Array, col: Int8Array, lo: number, hi: number): Uint32Array {
  let w = 0
  const out = rows.slice()
  for (let r = 0; r < rows.length; r++) {
    const v = col[rows[r]]
    if (v !== -128 && v >= lo && v <= hi) out[w++] = rows[r]
  }
  return out.slice(0, w)
}

function f32Key(x: number, buf: DataView): number {
  if (Number.isNaN(x)) return 0xffffffff
  buf.setFloat32(0, x === 0 ? 0 : x, true)
  const b = buf.getUint32(0, true)
  return (b & 0x80000000 ? ~b : b | 0x80000000) >>> 0
}

/** Stable LSD radix sort by (i8 key, f32 key), 16-bit digits, like overact-core. */
export function radixSort(rows: Uint32Array, age: Int8Array, income: Float32Array): Uint32Array {
  const n = rows.length
  let idx = rows.slice()
  let tmpIdx = new Uint32Array(n)
  let keys = new Uint32Array(n)
  let tmpKeys = new Uint32Array(n)
  const hist = new Uint32Array(65536)
  const dv = new DataView(new ArrayBuffer(4))
  const pass = (shift: number) => {
    hist.fill(0)
    for (let j = 0; j < n; j++) hist[(keys[j] >>> shift) & 0xffff]++
    for (let h = 0; h < 65536; h++) if (hist[h] === n) return
    let sum = 0
    for (let h = 0; h < 65536; h++) {
      const c = hist[h]
      hist[h] = sum
      sum += c
    }
    for (let j = 0; j < n; j++) {
      const k = keys[j]
      const o = hist[(k >>> shift) & 0xffff]++
      tmpKeys[o] = k
      tmpIdx[o] = idx[j]
    }
    ;[keys, tmpKeys] = [tmpKeys, keys]
    ;[idx, tmpIdx] = [tmpIdx, idx]
  }
  // Secondary key first (LSD), then primary.
  for (let j = 0; j < n; j++) keys[j] = f32Key(income[idx[j]], dv)
  pass(0)
  pass(16)
  for (let j = 0; j < n; j++) {
    const v = age[idx[j]]
    keys[j] = v === -128 ? 0xffff : v + 127
  }
  pass(0)
  return idx
}

/** Mean-colour raster, same algorithm as overact-core (difference arrays per row). */
export function raster(
  offsets: Uint32Array,
  segments: Uint16Array,
  axisLen: number,
  rows: Uint32Array,
  width: number,
  height: number,
  k: number,
  colors: Float32Array,
  out: Uint8ClampedArray,
) {
  const diff = new Float32Array(3 * (width + 1))
  const direct = new Float32Array(3 * (width + 1))
  const stride = width + 1
  const scale = width / axisLen
  const step = rows.length / height
  for (let y = 0; y < height; y++) {
    const lo = Math.floor(y * step)
    const hi = Math.min(rows.length, Math.floor((y + 1) * step))
    const cnt = hi - lo
    if (cnt <= 0) continue
    diff.fill(0)
    direct.fill(0)
    const ks = cnt <= k ? cnt : k
    for (let j = 0; j < ks; j++) {
      const p = rows[lo + Math.floor(((2 * j + 1) * cnt) / (2 * ks))]
      const a = offsets[p]
      const b = offsets[p + 1]
      for (let s = a; s < b; s++) {
        const seg = segments[s]
        const ts = seg >>> 5
        const te = s + 1 < b ? segments[s + 1] >>> 5 : axisLen
        const code = seg & 31
        const xs = ts * scale
        const xe = te * scale
        const is = Math.floor(xs)
        const ie = Math.floor(xe)
        for (let ch = 0; ch < 3; ch++) {
          const c = colors[code * 3 + ch]
          const o = ch * stride
          if (is === ie) direct[o + is] += (xe - xs) * c
          else {
            direct[o + is] += (is + 1 - xs) * c
            diff[o + is + 1] += c
            diff[o + ie] -= c
            direct[o + ie] += (xe - ie) * c
          }
        }
      }
    }
    const inv = 1 / ks
    const r = [0, 0, 0]
    for (let x = 0; x < width; x++) {
      for (let ch = 0; ch < 3; ch++) {
        r[ch] += diff[ch * stride + x]
        out[(y * width + x) * 4 + ch] = Math.sqrt((r[ch] + direct[ch * stride + x]) * inv) * 255
      }
      out[(y * width + x) * 4 + 3] = 255
    }
  }
}
