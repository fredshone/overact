// Types shared by the main thread and the data worker. They mirror the JSON
// accepted by overact-core (see crates/overact-core/src/{filter,sort,features}.rs).

export type DType = 'u8' | 'u16' | 'u32' | 'i8' | 'i16' | 'i32' | 'f32' | 'f64' | 'utf8'
export type ColumnKind = 'numeric' | 'categorical' | 'boolean'

export interface BlobRef {
  dtype: DType
  byteOffset: number
  byteLength: number
}

export interface ColumnMeta extends BlobRef {
  name: string
  kind: ColumnKind
  nullValue: number | null
  nullCount?: number
  min: number | null
  max: number | null
  categories?: string[]
}

export interface Header {
  format: 'overact'
  version: number
  n_persons: number
  n_segments: number
  time_axis: { start: number; end: number; unit?: string }
  activities: { code: number; label: string }[]
  blobs: {
    offsets: BlobRef
    segments: BlobRef
    person_id_offsets?: BlobRef
    person_id_data?: BlobRef
  }
  columns: ColumnMeta[]
  meta: Record<string, unknown>
}

export type Measure = 'count' | 'start' | 'duration' | 'end'

/**
 * Derived schedule features. Times are minutes from the axis start.
 * `measure`: a measure of the episodes of `activity` (null: any; with `not`:
 * any other), either the `nth` (1 first, 2 second, -1 last) or all (0).
 */
export type Feature =
  | { feature: 'measure'; measure: Measure; activity: number | null; not: boolean; nth: number }
  | { feature: 'distinct' }
  | { feature: 'doing_at'; activity: number; minute: number }
  | { feature: 'doing_during'; activity: number; start: number; end: number; all: boolean }

/**
 * Older names for measures, still accepted from saved views and links. The
 * engine only takes `Feature`: `normalizeFeature` rewrites these on load.
 */
export type LegacyFeature =
  | { feature: 'minutes'; activity: number }
  | { feature: 'first_start'; activity: number }
  | { feature: 'last_end'; activity: number }
  | { feature: 'episodes'; activity: number }
  | { feature: 'segments' }

export type ColRef = { column: number } | Feature

export type Test =
  | { type: 'range'; min: number | null; max: number | null }
  | { type: 'in'; values: number[]; null?: boolean }
  | { type: 'is_null' }

export type Condition = { col: ColRef; negate: boolean } & Test

export interface SortKey {
  col: ColRef
  desc: boolean
  order?: number[] | null
}

/** Keep `n` of the filtered persons, chosen at random (by `seed`). */
export interface Sample {
  n: number
  seed: number
}

export interface ViewSpec {
  filter: Condition[]
  sort: SortKey[]
  sample?: Sample | null
}

export interface ViewInfo {
  /** Rows shown (after sampling). */
  count: number
  /** Persons passing the filter, before sampling. */
  matched: number
  version: number
}

export interface PersonInfo {
  index: number
  id: string | null
  segments: [number, number, number][]
}

export interface DrawParams {
  width: number
  height: number
  rowStart: number
  rowEnd: number
  t0: number
  t1: number
  maxSamples: number
  mode: RenderMode
}

/** 0 mean colour, 1 majority activity, 2 first row only (see `RasterMode`). */
export type RenderMode = 0 | 1 | 2

export type Source = { kind: 'file'; file: File } | { kind: 'url'; url: string }

export interface LoadOptions {
  /** Load roughly this fraction of persons (1 = all), chosen at random. */
  fraction: number
  seed: number
  /** Use the multi-threaded engine when the page allows it (default true). */
  threads?: boolean
}

/** Activities table plus optional attributes table (CSV, TSV or Parquet). */
export interface TableFiles {
  schedules: File
  attributes?: File | null
}

/** Column roles and options; mirrors `overact_ingest::Config` (snake_case). */
export interface TableMapping {
  id_col: string
  act_col: string
  start_col: string
  /** Exactly one of end_col / duration_col. */
  end_col?: string | null
  duration_col?: string | null
  seq_col?: string | null
  /** Person id column of the attributes table (default: id_col). */
  attr_id_col?: string | null
  axis: [number, number]
  time_unit: 'minute' | 'second' | 'hour'
  /** Attribute columns to leave out. */
  exclude: string[]
}

export type ColType = 'text' | 'int' | 'float' | 'bool' | 'unsupported'

/** A first look at a table: see `overact_ingest::inspect`. */
export interface Inspection {
  format: 'csv' | 'parquet'
  columns: { name: string; type: ColType }[]
  preview: (string | null)[][]
  n_rows: number | null
  bytes: number
}

// Messages main -> worker
export type Request =
  | { type: 'open'; source: Source; options: LoadOptions }
  | { type: 'inspect'; file: File }
  | { type: 'openTables'; tables: TableFiles; mapping: TableMapping; options: LoadOptions }
  | { type: 'saveOveract' }
  | { type: 'palette'; colors: number[]; background: number }
  | { type: 'submit'; view: number; spec: ViewSpec }
  | { type: 'dropView'; view: number }
  | { type: 'attachCanvas'; view: number; canvas: OffscreenCanvas }
  | { type: 'draw'; view: number; params: DrawParams; exact: boolean }
  | { type: 'profile'; view: number }
  | { type: 'gutter'; view: number; col: ColRef; height: number; rowStart: number; rowEnd: number }
  | { type: 'hover'; view: number; pos: number; cols: ColRef[] }
  | { type: 'columnRange'; col: ColRef }
  | { type: 'exportIds'; view: number; format: 'csv' | 'parquet' }
  | { type: 'snapshot'; view: number }
  | { type: 'stats' }

export type WorkerEvent =
  | { type: 'progress'; phase: string; done: number; total: number }
  | { type: 'jobProgress'; pending: number[]; progress: number }
  | { type: 'viewReady'; view: number; info: ViewInfo; ms: number }
  | { type: 'profileReady'; view: number }
  | { type: 'drawn'; view: number; ms: number; rasterMs?: number; exact: boolean }
  | { type: 'error'; message: string }
