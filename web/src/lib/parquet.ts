// A minimal Parquet writer for exporting person ids: required UTF-8 and
// INT32 columns, PLAIN encoding, no compression, one row group. Enough for
// pandas / polars / pyarrow to read back.

/** A UTF-8 column is either strings or packed bytes: string `i` is `data[offsets[i]..offsets[i + 1]]`. */
type Col =
  | { name: string; kind: 'utf8'; values: string[] }
  | { name: string; kind: 'utf8'; offsets: Uint32Array; data: Uint8Array }
  | { name: string; kind: 'int32'; values: Int32Array | number[] }

function rows(c: Col): number {
  return 'offsets' in c ? c.offsets.length - 1 : c.values.length
}

class Bytes {
  private chunks: Uint8Array[] = []
  private cur = new Uint8Array(1 << 16)
  private pos = 0
  length = 0

  private ensure(n: number) {
    if (this.pos + n <= this.cur.length) return
    this.chunks.push(this.cur.subarray(0, this.pos))
    this.cur = new Uint8Array(Math.max(1 << 16, n))
    this.pos = 0
  }
  byte(b: number) {
    this.ensure(1)
    this.cur[this.pos++] = b
    this.length++
  }
  bytes(b: Uint8Array) {
    this.ensure(b.length)
    this.cur.set(b, this.pos)
    this.pos += b.length
    this.length += b.length
  }
  u32(v: number) {
    this.ensure(4)
    new DataView(this.cur.buffer, this.cur.byteOffset).setUint32(this.pos, v, true)
    this.pos += 4
    this.length += 4
  }
  i32(v: number) {
    this.u32(v >>> 0)
  }
  varint(v: bigint) {
    let x = v
    do {
      let b = Number(x & 0x7fn)
      x >>= 7n
      if (x) b |= 0x80
      this.byte(b)
    } while (x)
  }
  parts(): Uint8Array[] {
    return [...this.chunks, this.cur.subarray(0, this.pos)]
  }
}

// --- Thrift compact protocol (just what Parquet metadata needs) ---

const T = { I32: 5, I64: 6, BINARY: 8, LIST: 9, STRUCT: 12 } as const
type Field = [id: number, type: number, value: unknown]

class Thrift {
  constructor(private out: Bytes) {}
  private zigzag(v: bigint) {
    return (v << 1n) ^ (v >> 63n)
  }
  struct(fields: Field[]) {
    let last = 0
    for (const [id, type, value] of fields) {
      if (value === undefined) continue
      const delta = id - last
      if (delta > 0 && delta <= 15) this.out.byte((delta << 4) | type)
      else {
        this.out.byte(type)
        this.out.varint(this.zigzag(BigInt(id)))
      }
      last = id
      this.value(type, value)
    }
    this.out.byte(0)
  }
  private value(type: number, v: unknown) {
    switch (type) {
      case T.I32:
      case T.I64:
        this.out.varint(this.zigzag(BigInt(v as number | bigint)) & ((1n << 64n) - 1n))
        break
      case T.BINARY: {
        const b = new TextEncoder().encode(v as string)
        this.out.varint(BigInt(b.length))
        this.out.bytes(b)
        break
      }
      case T.LIST: {
        const [elemType, items] = v as [number, unknown[]]
        if (items.length < 15) this.out.byte((items.length << 4) | elemType)
        else {
          this.out.byte(0xf0 | elemType)
          this.out.varint(BigInt(items.length))
        }
        for (const it of items) this.value(elemType, it)
        break
      }
      case T.STRUCT:
        this.struct(v as Field[])
        break
    }
  }
}

const TYPE = { INT32: 1, BYTE_ARRAY: 6 }
const UTF8 = 0
const PAGE_ROWS = 1 << 20

/** Builds a Parquet file. Returns it as Blob parts. */
export function writeParquet(columns: Col[], createdBy = 'overact'): Blob {
  const n = columns.length ? rows(columns[0]) : 0
  const out = new Bytes()
  out.bytes(new TextEncoder().encode('PAR1'))
  const chunks: Field[][] = []
  const enc = new TextEncoder()
  for (const col of columns) {
    const start = out.length
    let total = 0
    for (let a = 0; a < n || (a === 0 && n === 0); a += PAGE_ROWS) {
      const b = Math.min(n, a + PAGE_ROWS)
      const page = new Bytes()
      if ('offsets' in col) {
        for (let i = a; i < b; i++) {
          page.u32(col.offsets[i + 1] - col.offsets[i])
          page.bytes(col.data.subarray(col.offsets[i], col.offsets[i + 1]))
        }
      } else if (col.kind === 'utf8') {
        for (let i = a; i < b; i++) {
          const s = enc.encode(col.values[i])
          page.u32(s.length)
          page.bytes(s)
        }
      } else {
        for (let i = a; i < b; i++) page.i32(col.values[i])
      }
      const header = new Bytes()
      new Thrift(header).struct([
        [1, T.I32, 0], // DATA_PAGE
        [2, T.I32, page.length],
        [3, T.I32, page.length],
        [5, T.STRUCT, [
          [1, T.I32, b - a],
          [2, T.I32, 0], // PLAIN
          [3, T.I32, 3], // RLE (unused: required column)
          [4, T.I32, 3],
        ]],
      ])
      for (const p of header.parts()) out.bytes(p)
      for (const p of page.parts()) out.bytes(p)
      total += header.length + page.length
      if (n === 0) break
    }
    chunks.push([
      [2, T.I64, start],
      [3, T.STRUCT, [
        [1, T.I32, col.kind === 'utf8' ? TYPE.BYTE_ARRAY : TYPE.INT32],
        [2, T.LIST, [T.I32, [0]]], // PLAIN
        [3, T.LIST, [T.BINARY, [col.name]]],
        [4, T.I32, 0], // UNCOMPRESSED
        [5, T.I64, n],
        [6, T.I64, total],
        [7, T.I64, total],
        [9, T.I64, start],
      ]],
    ])
  }
  const schema: Field[][] = [
    [
      [4, T.BINARY, 'schema'],
      [5, T.I32, columns.length],
    ],
    ...columns.map((c): Field[] => [
      [1, T.I32, c.kind === 'utf8' ? TYPE.BYTE_ARRAY : TYPE.INT32],
      [3, T.I32, 0], // REQUIRED
      [4, T.BINARY, c.name],
      [6, T.I32, c.kind === 'utf8' ? UTF8 : undefined],
    ]),
  ]
  const meta = new Bytes()
  new Thrift(meta).struct([
    [1, T.I32, 1],
    [2, T.LIST, [T.STRUCT, schema]],
    [3, T.I64, n],
    [4, T.LIST, [T.STRUCT, [[
      [1, T.LIST, [T.STRUCT, chunks]],
      [2, T.I64, out.length - 4],
      [3, T.I64, n],
    ]]]],
    [6, T.BINARY, createdBy],
  ])
  const metaLen = meta.length
  for (const p of meta.parts()) out.bytes(p)
  out.u32(metaLen)
  out.bytes(new TextEncoder().encode('PAR1'))
  return new Blob(out.parts() as BlobPart[], { type: 'application/vnd.apache.parquet' })
}
