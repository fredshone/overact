// Application state (Svelte 5 runes) and the actions that change it.

import { EngineClient, type OpenResult } from './engine/client'
import type {
  Condition,
  Header,
  Inspection,
  LoadOptions,
  RenderMode,
  SortKey,
  Source,
  TableFiles,
  TableMapping,
  ViewSpec,
} from './engine/types'
import { colMeta, uniqueTitle } from './format'
import { defaultColors, hexToInt } from './palette'
import { incompatibility, remapProfile, toUnionCodes, translateFilter, translateSort, unionHeader } from './datasets'
import {
  decodeHash,
  fingerprint,
  nameCond,
  nameSort,
  resolveCond,
  resolveSort,
  type NamedCondition,
  type ViewConfig,
} from './viewconfig'

export const MAX_WINDOWS = 6

/** How a dataset first opens: a sort, and windows whose filters name columns (resolved on load). */
export interface Preset {
  sort?: SortKey[]
  windows?: { title: string; filter: NamedCondition[] }[]
}
/** Choices for samples per pixel row. */
export const SAMPLE_CHOICES = [4, 8, 16, 32, 64, 128]

/** A `#rrggbb` colour (what the colour inputs and defaults produce). */
export function isColor(c: unknown): c is string {
  return typeof c === 'string' && /^#[0-9a-f]{6}$/i.test(c)
}

/** A loaded dataset, with its own engine (worker and WASM memory). */
export interface Dataset {
  id: number
  name: string
  client: EngineClient
  /** Header as the engine sees it (subset counts). */
  header: Header
  fileHeader: Header
  loadMs: number
  threads: number
  /** Total minutes in each activity over all persons, by code. */
  activityMinutes: number[]
  /** Union (legend) code of each of this dataset's activity codes. */
  toUnion: number[]
  off: () => void
}

export interface Win {
  id: number
  /** Id of the dataset shown. */
  dataset: number
  title: string
  filter: Condition[]
  /** Rows shown (after sampling). */
  count: number | null
  /** Persons passing the filter, before sampling. */
  matched: number | null
  version: number
  busy: boolean
  progress: number
  profile: Uint32Array | null
  /** Time of the last filter/sort job, ms. */
  jobMs: number
  /** The rows changed and `profile` still shows the previous ones. */
  profilePending: boolean
}

export interface ActivityStyle {
  code: number
  label: string
  color: string
  visible: boolean
}

export type VerticalMode = 'fit' | 'rows'
export type Theme = 'system' | 'light' | 'dark'

/** A message shown at the bottom of the screen until dismissed or timed out. */
export interface Notice {
  id: number
  text: string
  kind: 'error' | 'info'
}

/** Narrowest time crop, in minutes (zooming in further stops here). */
export const MIN_SPAN = 10

const darkQuery = typeof matchMedia !== 'undefined' ? matchMedia('(prefers-color-scheme: dark)') : null

/** Colours read from the theme's CSS variables (canvases cannot use variables). */
export interface ThemeColors {
  fg: string
  accent: string
  canvasBg: string
}

const COLOR_KEY = 'overact.colors'

function loadStoredColors(): Record<string, string> {
  try {
    return JSON.parse(localStorage.getItem(COLOR_KEY) ?? '{}')
  } catch {
    return {}
  }
}

/** Remembers colours the user changed (by label); defaults are not stored. */
function storeColors(acts: ActivityStyle[]) {
  try {
    const all = loadStoredColors()
    const defaults = defaultColors(acts.map((a) => a.label))
    acts.forEach((a, i) => {
      if (a.color.toLowerCase() === defaults[i].toLowerCase()) delete all[a.label]
      else all[a.label] = a.color
    })
    localStorage.setItem(COLOR_KEY, JSON.stringify(all))
  } catch {
    /* storage unavailable */
  }
}

class AppState {
  /** Loaded datasets, in load order. */
  datasets = $state.raw<Dataset[]>([])
  /** The Loader is shown over the windows to add a dataset. */
  adding = $state(false)
  /** State of the load in progress (or the last one, if it failed). */
  status = $state<'empty' | 'loading' | 'ready' | 'error'>('empty')
  error = $state('')
  notices = $state<Notice[]>([])
  private nextNotice = 1
  progress = $state({ phase: '', done: 0, total: 0 })
  /**
   * Union of the datasets' headers (the dataset's own with one dataset).
   * The shared sort, legend and time axis refer to it.
   */
  header = $state.raw<Header | null>(null)
  /** Name of the source being (or last) loaded. */
  sourceName = $state('')
  /** WASM memory in bytes, by dataset id. */
  memoryOf = $state<Record<number, number>>({})
  drawMs = $state(0)
  threads = $state(1)
  /** Total minutes in each activity over all datasets, by union code. */
  activityMinutes = $state.raw<number[]>([])

  activities = $state<ActivityStyle[]>([])
  /** Optional crop of the time axis (minutes from the axis start), for all windows. */
  crop = $state({ on: false, start: 0, end: 1440 })
  /**
   * Vertical layout. Fit: each window's rows fill its height. Rows: every
   * window has the rows-per-pixel of the largest one. `pos` (0..1) is the
   * scroll position through the rows when only some of them are shown.
   */
  vertical = $state({ mode: 'fit' as VerticalMode, pos: 0 })
  /** Rows shown per window (null: all), scrolled with `vertical.pos`. */
  show = $state<number | null>(null)
  /** Random subsample of each window's filtered persons (n null: none). */
  sample = $state<{ n: number | null; seed: number }>({ n: null, seed: 0 })
  /** Sort keys, shared by all windows. */
  sort = $state<SortKey[]>([])
  /** Value range of the primary sort key (for the gutter ramp), if known. */
  sortRange = $state<{ min: number; max: number } | null>(null)
  private rangeReq = 0
  /** Height of the profile strips, CSS px. */
  profileHeight = $state(108)
  windows = $state<Win[]>([])
  hover = $state<{ minute: number | null; win: number | null }>({ minute: null, win: null })
  /** A time range being brushed on a window's axis (minutes from the axis start). */
  brush = $state<{ win: number; a: number; b: number } | null>(null)
  renderMode = $state<RenderMode>(0)
  maxSamples = $state(32)
  exactPass = $state(true)
  theme = $state<Theme>('system')
  /** Whether the OS prefers dark (followed when `theme` is 'system'). */
  systemDark = $state(darkQuery?.matches ?? false)
  /** Device pixels per CSS pixel (capped at 2), updated when it changes. */
  dpr = $state(typeof devicePixelRatio === 'number' ? Math.min(2, devicePixelRatio || 1) : 1)
  colors = $state.raw<ThemeColors>({ fg: '#1d1d1f', accent: '#2f5bd3', canvasBg: '#ffffff' })
  showHelp = $state(false)
  showProfiles = $state(true)
  /** True while the user is scrolling/zooming/dragging: draw with fewer samples. */
  interacting = $state(false)
  private interactTimer: ReturnType<typeof setTimeout> | undefined
  /** Height in CSS px of each window's body, by window id. */
  windowHeights = $state<Record<number, number>>({})

  /** Height in CSS px of the tallest window body (for keyboard scrolling). */
  get windowHeight(): number {
    return Math.max(1, ...Object.values(this.windowHeights))
  }

  get resolvedTheme(): 'light' | 'dark' {
    return this.theme === 'system' ? (this.systemDark ? 'dark' : 'light') : this.theme
  }

  constructor() {
    darkQuery?.addEventListener('change', (e) => (this.systemDark = e.matches))
    this.watchDpr()
  }

  /** Follows devicePixelRatio (e.g. a window dragged to another monitor). */
  private watchDpr() {
    if (typeof matchMedia === 'undefined' || typeof devicePixelRatio !== 'number') return
    const q = matchMedia(`(resolution: ${devicePixelRatio}dppx)`)
    q.addEventListener(
      'change',
      () => {
        this.dpr = Math.min(2, devicePixelRatio || 1)
        this.watchDpr()
      },
      { once: true },
    )
  }

  cycleTheme() {
    this.theme = this.theme === 'system' ? 'dark' : this.theme === 'dark' ? 'light' : 'system'
  }

  /** Shows a message; errors stay longer than notes. */
  notify(text: string, kind: Notice['kind'] = 'error') {
    const id = this.nextNotice++
    this.notices = [...this.notices.filter((n) => n.text !== text), { id, text, kind }].slice(-4)
    setTimeout(() => this.dismiss(id), kind === 'error' ? 12_000 : 5_000)
  }

  dismiss(id: number) {
    this.notices = this.notices.filter((n) => n.id !== id)
  }

  private nextWinId = 1
  /** A config from the URL hash, applied when a dataset opens. */
  pendingConfig: ViewConfig | null = typeof location !== 'undefined' ? decodeHash(location.hash) : null

  get axisLen(): number {
    const h = this.header
    return h ? h.time_axis.end - h.time_axis.start : 1440
  }

  get memory(): number {
    return Object.values(this.memoryOf).reduce((a, b) => a + b, 0)
  }

  datasetOf(w: Win): Dataset {
    return this.datasets.find((d) => d.id === w.dataset) ?? this.datasets[0]
  }

  /** The shared sort for one dataset's windows (keys it lacks are left out). */
  sortFor(ds: Dataset): SortKey[] {
    return translateSort(this.header!, ds.header, $state.snapshot(this.sort) as SortKey[]).keys
  }

  /** Names of the datasets that lack sort key `i`. */
  sortMissing(i: number): string[] {
    const k = $state.snapshot(this.sort[i]) as SortKey | undefined
    if (!k || this.datasets.length < 2) return []
    return this.datasets.filter((d) => translateSort(this.header!, d.header, [k]).missing.length).map((d) => d.name)
  }

  /** Visible time range in minutes from the axis start: the crop, or the whole axis. */
  get time(): { t0: number; t1: number } {
    return this.crop.on ? { t0: this.crop.start, t1: this.crop.end } : { t0: 0, t1: this.axisLen }
  }

  specOf(w: Win): ViewSpec {
    const n = this.sample.n
    return {
      filter: $state.snapshot(w.filter) as Condition[],
      sort: this.sortFor(this.datasetOf(w)),
      sample: n ? { n, seed: this.sample.seed } : null,
    }
  }

  /** Opens a packed dataset with an initial sort and windows (a saved view in the URL wins). */
  async open(source: Source, name: string, options: LoadOptions = { fraction: 1, seed: 1 }, preset: Preset = {}) {
    await this.load(name, (c) => c.open(source, options), preset)
  }

  /** Loads CSV/Parquet tables straight into the engine (no packing step). */
  async openTables(tables: TableFiles, mapping: TableMapping, options: LoadOptions = { fraction: 1, seed: 1 }) {
    await this.load(tables.schedules.name, (c) => c.openTables(tables, mapping, options))
  }

  /** A worker for looking at tables before loading them (kept until a load). */
  private inspector: EngineClient | null = null
  /** The engine of the load in progress. */
  private loading: EngineClient | null = null
  private nextDatasetId = 1

  inspect(file: File): Promise<Inspection> {
    this.inspector ??= new EngineClient()
    return this.inspector.inspect(file)
  }

  /** Shows the Loader to add another dataset. */
  startAdding() {
    this.adding = true
    this.status = 'empty'
    this.error = ''
  }

  /** Leaves the Loader (stopping a load in progress) and returns to the windows. */
  cancelAdding() {
    this.loading?.terminate()
    this.loading = null
    this.adding = false
    this.status = 'ready'
  }

  /**
   * Loads a dataset: in addition to the loaded ones while `adding`, else in
   * place of them.
   */
  private async load(name: string, run: (c: EngineClient) => Promise<OpenResult>, preset: Preset = {}) {
    this.inspector?.terminate()
    this.inspector = null
    this.loading?.terminate()
    if (!this.adding) this.close()
    const client = new EngineClient()
    this.loading = client
    this.status = 'loading'
    this.error = ''
    this.sourceName = name
    this.progress = { phase: 'header', done: 0, total: 1 }
    const off = client.on((ev) => this.onEvent(ev, client))
    const fail = (msg: string) => {
      off()
      client.terminate()
      this.loading = null
      this.status = 'error'
      this.error = msg
    }
    let res: OpenResult
    try {
      res = await run(client)
    } catch (e) {
      if (this.loading === client) fail(e instanceof Error ? e.message : String(e))
      return
    }
    if (this.loading !== client) return
    const why = this.datasets.length ? incompatibility(this.datasets[0].header, res.header) : null
    if (why) return fail(why)
    this.loading = null
    const first = !this.datasets.length
    const ds: Dataset = {
      id: this.nextDatasetId++,
      name: this.datasets.some((d) => d.name === name) ? uniqueTitle(name, this.datasets.map((d) => d.name)) : name,
      client,
      header: res.header,
      fileHeader: res.fileHeader,
      loadMs: res.loadMs,
      threads: res.threads,
      activityMinutes: res.activityMinutes,
      toUnion: [],
      off,
    }
    this.memoryOf[ds.id] = res.memory
    this.threads = res.threads
    this.setDatasets([...this.datasets, ds])
    this.adding = false
    this.status = 'ready'
    if (!first) {
      // Compare: a window on the new dataset, like the first window.
      const id = this.addWindow(this.windows[0], ds.id)
      if (id === undefined) this.notify(`Loaded "${ds.name}". Pick it in a window's dataset menu.`, 'info')
      return
    }
    this.crop = { on: false, start: 0, end: this.axisLen }
    this.vertical = { mode: this.vertical.mode, pos: 0 }
    this.show = null
    this.sample = { n: null, seed: 0 }
    this.sort = preset.sort ?? []
    this.nextWinId = 1
    this.windowHeights = {}
    this.readThemeColors()
    this.refreshSortRange()
    await this.pushPalette()
    const cfg = this.pendingConfig
    this.pendingConfig = null
    if (cfg) {
      try {
        this.applyConfig(cfg)
        return
      } catch (e) {
        this.notify(`Could not apply the saved view: ${e instanceof Error ? e.message : e}`)
      }
    }
    let wins: { title: string; filter: Condition[] }[] = []
    try {
      wins = (preset.windows ?? []).map((w) => ({ title: w.title, filter: w.filter.map((c) => resolveCond(res.header, c)) }))
    } catch (e) {
      this.notify(`Could not set up the windows: ${e instanceof Error ? e.message : e}`)
    }
    if (!wins.length) {
      this.addWindow()
      return
    }
    for (const w of wins.slice(0, MAX_WINDOWS)) {
      const id = this.addWindow()!
      const win = this.windows.find((x) => x.id === id)!
      win.title = w.title
      win.filter = w.filter
    }
  }

  /**
   * Replaces the dataset list: rebuilds the union header, legend and
   * activity maps, and moves the shared sort to the new union by name.
   */
  private setDatasets(list: Dataset[]) {
    const old = this.header
    const union = list.length ? unionHeader(list.map((d) => d.header)) : null
    for (const d of list) d.toUnion = union ? toUnionCodes(d.header, union) : []
    this.datasets = list
    this.header = union
    if (!union) {
      this.activities = []
      this.activityMinutes = []
      return
    }
    // Keep colours and visibility (by label) of activities already shown.
    const stored = loadStoredColors()
    const defaults = defaultColors(union.activities.map((a) => a.label))
    this.activities = union.activities.map((a, i) => {
      const prev = this.activities.find((x) => x.label === a.label)
      return {
        code: a.code,
        label: a.label,
        color: prev?.color ?? (isColor(stored[a.label]) ? stored[a.label] : defaults[i]),
        visible: prev?.visible ?? true,
      }
    })
    const minutes = new Array<number>(union.activities.length).fill(0)
    for (const d of list) d.activityMinutes.forEach((m, c) => (minutes[d.toUnion[c]] += m))
    this.activityMinutes = minutes
    if (old && old !== union) {
      const { keys, missing } = translateSort(old, union, $state.snapshot(this.sort) as SortKey[])
      this.sort = keys
      if (missing.length) this.submitAll()
      this.refreshSortRange()
      for (const w of this.windows) this.fetchProfile(w)
      this.pushPalette()
    }
  }

  /** Closes one dataset and its windows (all datasets: back to the loader). */
  removeDataset(id: number) {
    const ds = this.datasets.find((d) => d.id === id)
    if (!ds) return
    if (this.datasets.length === 1) return this.close()
    for (const w of this.windows.filter((w) => w.dataset === id)) {
      this.windows = this.windows.filter((x) => x.id !== w.id)
      delete this.windowHeights[w.id]
    }
    ds.off()
    ds.client.terminate()
    delete this.memoryOf[id]
    this.setDatasets(this.datasets.filter((d) => d.id !== id))
    if (!this.windows.length) this.addWindow()
  }

  /** Shows another dataset in window `w`, keeping the filter conditions that fit it. */
  setWindowDataset(w: Win, dataset: number) {
    if (w.dataset === dataset) return
    const at = this.windows.indexOf(w)
    // The canvas belongs to the old dataset's worker: replace the window.
    const id = this.addWindow(w, dataset, true)
    if (id === undefined) return
    const nw = this.windows.pop()!
    nw.title = w.title
    this.windows = this.windows.map((x, i) => (i === at ? nw : x))
    delete this.windowHeights[w.id]
    this.datasetOf(w).client.dropView(w.id)
  }

  toConfig(): ViewConfig {
    const h = this.header!
    const about = (d: Dataset) => ({ name: d.name, persons: d.header.n_persons, fingerprint: fingerprint(d.header) })
    return {
      overact: 1,
      dataset: about(this.datasets[0]),
      ...(this.datasets.length > 1 ? { datasets: this.datasets.map(about) } : {}),
      crop: { ...this.crop },
      colors: Object.fromEntries(this.activities.map((a) => [a.label, a.color])),
      hidden: this.activities.filter((a) => !a.visible).map((a) => a.label),
      vertical: { ...this.vertical },
      show: this.show,
      sample: { ...this.sample },
      renderMode: this.renderMode,
      maxSamples: this.maxSamples,
      profileHeight: this.profileHeight,
      sort: this.sort.map((k) => nameSort(h, $state.snapshot(k) as SortKey)),
      windows: this.windows.map((w) => {
        const d = this.datasetOf(w)
        const i = this.datasets.indexOf(d)
        return {
          title: w.title,
          filter: w.filter.map((c) => nameCond(d.header, $state.snapshot(c) as Condition)),
          ...(i > 0 ? { dataset: i } : {}),
        }
      }),
    }
  }

  /** Applies a saved view. Throws (changing nothing) if it does not fit the dataset. */
  applyConfig(cfg: ViewConfig) {
    const h = this.header!
    // Resolve everything first so a bad config changes nothing.
    // Older configs had per-window sorts that could be linked.
    const named = cfg.sort ?? (cfg.linkSort === false ? cfg.windows[0]?.sort : cfg.sharedSort) ?? []
    const sort = named.map((k) => resolveSort(h, k))
    // Older configs used "All persons" / "Window N" as default titles.
    const legacy = cfg.sort === undefined
    const oldDefault = (t: string) => legacy && /^(All persons|Window \d+)$/.test(t)
    // Windows on datasets that are not loaded are left out.
    const onLoaded = cfg.windows.filter((w) => (w.dataset ?? 0) < this.datasets.length)
    const skipped = cfg.windows.length - onLoaded.length
    const wins = onLoaded.slice(0, MAX_WINDOWS).map((w) => {
      const d = this.datasets[Number.isSafeInteger(w.dataset) && w.dataset! > 0 ? w.dataset! : 0]
      return {
        title: typeof w.title !== 'string' || oldDefault(w.title) ? '' : w.title,
        dataset: d.id,
        filter: w.filter.map((c) => resolveCond(d.header, c)),
      }
    })
    const L = this.axisLen
    const clampT = (t: number) => (Number.isFinite(t) ? Math.max(0, Math.min(L, t)) : 0)
    let crop = { on: false, start: 0, end: L }
    if (cfg.crop) crop = { on: cfg.crop.on, start: clampT(cfg.crop.start), end: clampT(cfg.crop.end) }
    else if (cfg.time && (cfg.time.t0 > 0 || cfg.time.t1 < L)) crop = { on: true, start: clampT(cfg.time.t0), end: clampT(cfg.time.t1) }
    if (!(crop.end > crop.start)) crop = { on: false, start: 0, end: L }
    // Configs come from files and shared links: keep only well-formed values.
    const colors = cfg.colors && typeof cfg.colors === 'object' ? cfg.colors : {}
    const hidden = Array.isArray(cfg.hidden) ? cfg.hidden : []
    const count = (n: unknown) => (Number.isSafeInteger(n) && (n as number) > 0 ? (n as number) : null)
    const pos = Number(cfg.vertical?.pos)
    const seed = Number(cfg.sample?.seed)
    // Everything is resolved: apply.
    for (const a of this.activities) {
      const c = colors[a.label]
      if (isColor(c)) a.color = c
      a.visible = !hidden.includes(a.label)
    }
    this.crop = crop
    this.vertical = { mode: cfg.vertical?.mode === 'rows' ? 'rows' : 'fit', pos: Number.isFinite(pos) ? Math.max(0, Math.min(1, pos)) : 0 }
    this.show = count(cfg.show)
    this.sample = { n: count(cfg.sample?.n), seed: Number.isSafeInteger(seed) ? seed >>> 0 : 0 }
    this.renderMode = cfg.renderMode === 1 || cfg.renderMode === 2 ? cfg.renderMode : 0
    if (SAMPLE_CHOICES.includes(cfg.maxSamples)) this.maxSamples = cfg.maxSamples
    if (Number.isFinite(cfg.profileHeight) && cfg.profileHeight! > 0) this.profileHeight = Math.min(2000, cfg.profileHeight!)
    this.sort = sort
    this.refreshSortRange()
    for (const w of this.windows) this.datasetOf(w).client.dropView(w.id)
    this.windows = []
    this.windowHeights = {}
    for (const w of wins.length ? wins : [{ title: 'Unnamed_0', dataset: this.datasets[0].id, filter: [] }]) {
      const id = this.addWindow(undefined, w.dataset)!
      const win = this.windows.find((x) => x.id === id)!
      if (w.title) win.title = w.title
      win.filter = w.filter
    }
    this.pushPalette()
    this.saveColors()
    if (skipped) {
      this.notify(`${skipped} window${skipped > 1 ? 's' : ''} of the view show${skipped > 1 ? '' : 's'} a dataset that is not loaded.`, 'info')
    }
    const saved = Array.isArray(cfg.datasets) ? cfg.datasets : [cfg.dataset]
    if (this.datasets.some((d, i) => saved[i] && fingerprint(d.header) !== saved[i]?.fingerprint)) {
      this.notify(`View saved for "${saved.map((d) => d?.name ?? 'another dataset').join('", "')}"; applied by column name.`, 'info')
    }
  }

  /** Reads the theme's colours (after a theme change). */
  readThemeColors() {
    const css = getComputedStyle(document.documentElement)
    const v = (name: string, dflt: string) => css.getPropertyValue(name).trim() || dflt
    this.colors = { fg: v('--fg', '#1d1d1f'), accent: v('--accent', '#2f5bd3'), canvasBg: v('--canvas-bg', '#ffffff') }
  }

  /**
   * Sends the activity colours to the engine and redraws. Hidden activities
   * are drawn in the background colour.
   */
  async pushPalette() {
    const bg = this.colors.canvasBg
    const colors = this.activities.map((a) => hexToInt(a.visible ? a.color : bg))
    await Promise.all(this.datasets.map((d) => d.client.palette(d.toUnion.map((u) => colors[u]), hexToInt(bg))))
    this.redrawToken++
  }

  private paletteFrame = 0
  /** `pushPalette` at most once per frame (while a colour picker is dragged). */
  schedulePalette() {
    if (this.paletteFrame) return
    this.paletteFrame = requestAnimationFrame(() => {
      this.paletteFrame = 0
      this.pushPalette()
    })
  }

  /** Remembers the colours the user changed, by activity label. */
  saveColors() {
    storeColors(this.activities)
  }

  /** Closes the dataset and returns to the loader. */
  close() {
    for (const d of this.datasets) {
      d.off()
      d.client.terminate()
    }
    this.memoryOf = {}
    this.setDatasets([])
    this.windows = []
    this.windowHeights = {}
    this.adding = false
    this.status = 'empty'
  }

  /** Bumped when everything must be redrawn (palette, theme). */
  redrawToken = $state(0)

  private onEvent(ev: import('./engine/types').WorkerEvent, client: EngineClient) {
    switch (ev.type) {
      case 'progress':
        if (client === this.loading) this.progress = { phase: ev.phase, done: ev.done, total: ev.total }
        break
      case 'viewReady': {
        const w = this.windows.find((w) => w.id === ev.view)
        if (!w) break
        // The profile follows (an unchanged filter reuses it: profileReady comes at once).
        if (ev.ms > 0) w.profilePending = true
        w.count = ev.info.count
        w.matched = ev.info.matched
        w.version = ev.info.version + 1
        w.busy = false
        w.progress = 1
        w.jobMs = ev.ms
        // A computed feature's range is known once a view used it.
        if (!this.sortRange) this.refreshSortRange()
        const ds = this.datasetOf(w)
        client.stats().then((s) => {
          if (this.memoryOf[ds.id] !== undefined) this.memoryOf[ds.id] = s.memory
        })
        break
      }
      case 'profileReady': {
        const w = this.windows.find((w) => w.id === ev.view)
        if (w) this.fetchProfile(w)
        break
      }
      case 'jobProgress':
        for (const w of this.windows) {
          if (ev.pending[0] === w.id) w.progress = ev.progress
        }
        break
      case 'drawn':
        if (!ev.exact) this.drawMs = ev.ms
        break
      case 'error':
        this.notify(ev.message)
        break
    }
  }

  /** Gets a window's profile, with activity codes moved to the union's. */
  private fetchProfile(w: Win) {
    const ds = this.datasetOf(w)
    ds.client.profile(w.id).then((p) => {
      const cur = this.windows.find((x) => x.id === w.id)
      if (!cur || !this.header) return
      if (p) cur.profile = remapProfile(p, ds.toUnion, this.header.activities.length, this.axisLen)
      cur.profilePending = false
    })
  }

  async submit(w: Win) {
    w.busy = true
    w.progress = 0
    try {
      await this.datasetOf(w).client.submit(w.id, this.specOf(w))
    } catch (e) {
      w.busy = false
      this.notify(e instanceof Error ? e.message : String(e))
    }
  }

  submitAll() {
    for (const w of this.windows) this.submit(w)
  }

  /**
   * Adds a window on `dataset` (default: that of `from`, else the first),
   * with the filter of `from` (translated by name to another dataset).
   * `replacing`: the window takes another's place, so the limit does not apply.
   */
  addWindow(from?: Win, dataset?: number, replacing = false) {
    if (this.windows.length >= MAX_WINDOWS && !replacing) return
    const ds = this.datasets.find((d) => d.id === (dataset ?? from?.dataset)) ?? this.datasets[0]
    let filter: Condition[] = []
    if (from) {
      const t = translateFilter(this.datasetOf(from).header, ds.header, structuredClone($state.snapshot(from.filter)) as Condition[])
      filter = t.filter
      if (t.dropped) this.notify(`${t.dropped} filter condition${t.dropped > 1 ? 's do' : ' does'} not apply to "${ds.name}" and ${t.dropped > 1 ? 'were' : 'was'} left out.`, 'info')
    }
    const id = this.nextWinId++
    const w: Win = {
      id,
      dataset: ds.id,
      title: uniqueTitle(from?.title ?? 'Unnamed', this.windows.map((w) => w.title)),
      filter,
      count: null,
      matched: null,
      version: 0,
      busy: true,
      progress: 0,
      profile: null,
      jobMs: 0,
      profilePending: false,
    }
    this.windows.push(w)
    // Submit after the window's canvas is attached (see Window.svelte).
    return id
  }

  renameWindow(id: number, title: string) {
    const w = this.windows.find((w) => w.id === id)
    if (w) w.title = title
  }

  /** Window being dragged to a new position, if any. */
  draggingWin = $state<number | null>(null)

  /** Moves window `id` to position `to` (0-based). */
  moveWindow(id: number, to: number) {
    const from = this.windows.findIndex((w) => w.id === id)
    to = Math.max(0, Math.min(this.windows.length - 1, to))
    if (from < 0 || from === to) return
    const ws = [...this.windows]
    const [w] = ws.splice(from, 1)
    ws.splice(to, 0, w)
    this.windows = ws
  }

  removeWindow(id: number) {
    const w = this.windows.find((w) => w.id === id)
    if (w) this.datasetOf(w).client.dropView(id)
    this.windows = this.windows.filter((w) => w.id !== id)
    delete this.windowHeights[id]
  }

  setFilter(w: Win, filter: Condition[]) {
    w.filter = filter
    this.submit(w)
  }

  setSort(sort: SortKey[]) {
    this.sort = sort
    this.refreshSortRange()
    this.submitAll()
  }

  /** Looks up the range of the primary sort key (from the header, or the engine for features). */
  refreshSortRange() {
    const key = this.sort[0]
    const req = ++this.rangeReq
    const h = this.header
    if (!key || !h) {
      this.sortRange = null
      return
    }
    const ok = (r: { min: number | null; max: number | null } | null) =>
      r && r.min !== null && r.max !== null ? { min: r.min, max: r.max } : null
    const meta = colMeta(h, key.col)
    if (meta) {
      this.sortRange = ok(meta)
      return
    }
    // A computed feature: its range over the datasets that have it.
    this.sortRange = null
    const asks = this.datasets.flatMap((d) => {
      const [k] = translateSort(h, d.header, [$state.snapshot(key) as SortKey]).keys
      return k ? [d.client.columnRange(k.col).catch(() => null)] : []
    })
    Promise.all(asks).then((rs) => {
      if (req !== this.rangeReq) return
      const got = rs.map(ok).filter((r) => r !== null)
      this.sortRange = got.length ? { min: Math.min(...got.map((r) => r.min)), max: Math.max(...got.map((r) => r.max)) } : null
    })
  }

  /** Samples `n` persons per window after filtering (null: all). */
  setSample(n: number | null) {
    if (n === this.sample.n) return
    this.sample = { ...this.sample, n }
    this.submitAll()
  }

  /** Draws a different random sample. */
  reshuffle() {
    this.sample = { ...this.sample, seed: (this.sample.seed + 1) >>> 0 }
    if (this.sample.n) this.submitAll()
  }

  setShow(n: number | null) {
    this.show = n
  }

  /** Crops the time axis of all windows to [start, end) minutes, or removes the crop. */
  setCrop(on: boolean, start = this.crop.start, end = this.crop.end) {
    const L = this.axisLen
    start = Math.max(0, Math.min(L - 1, start))
    end = Math.max(start + 1, Math.min(L, end))
    this.crop = { on, start, end }
  }

  /** Zooms the time axis by `factor` (< 1: in) keeping minute `at` in place. */
  zoomTime(factor: number, at?: number) {
    const L = this.axisLen
    const { t0, t1 } = this.time
    const c = at ?? (t0 + t1) / 2
    const span = Math.max(MIN_SPAN, Math.min(L, (t1 - t0) * factor))
    if (span >= L) return this.resetTime()
    let start = c - (c - t0) * (span / (t1 - t0))
    start = Math.max(0, Math.min(L - span, start))
    this.crop = { on: true, start, end: start + span }
  }

  /** Moves the cropped time range by `dm` minutes (clamped to the axis). */
  panTime(dm: number) {
    if (!this.crop.on) return
    const L = this.axisLen
    const span = this.crop.end - this.crop.start
    const start = Math.max(0, Math.min(L - span, this.crop.start + dm))
    this.crop = { on: true, start, end: start + span }
  }

  /** Crops to `[a, b)` minutes (in either order), at least MIN_SPAN wide. */
  zoomTo(a: number, b: number) {
    const L = this.axisLen
    let [s, e] = a < b ? [a, b] : [b, a]
    if (e - s < MIN_SPAN) {
      const c = (s + e) / 2
      s = c - MIN_SPAN / 2
      e = c + MIN_SPAN / 2
    }
    s = Math.max(0, s)
    e = Math.min(L, Math.max(e, s + MIN_SPAN))
    this.crop = { on: true, start: s, end: e }
  }

  resetTime() {
    this.crop = { ...this.crop, on: false }
  }

  copyFilter(from: Win, to: Win) {
    const t = translateFilter(this.datasetOf(from).header, this.datasetOf(to).header, structuredClone($state.snapshot(from.filter)) as Condition[])
    if (t.dropped) this.notify(`${t.dropped} filter condition${t.dropped > 1 ? 's do' : ' does'} not apply to "${to.title}" and ${t.dropped > 1 ? 'were' : 'was'} left out.`, 'info')
    this.setFilter(to, t.filter)
  }

  /** Rows of `w` per window height (`span`) out of the rows it scrolls through (`total`). */
  extent(w: Win): { span: number; total: number } {
    const n = w.count ?? 0
    const N = this.show
    if (this.vertical.mode === 'fit') return { span: N === null ? n : Math.min(N, n), total: n }
    const M = this.maxCount()
    return { span: N === null ? M : Math.min(N, M), total: M }
  }

  /** Visible rows `[first, end)` of a window. */
  rowRange(w: Win): [number, number] {
    const { span, total } = this.extent(w)
    const top = this.vertical.pos * Math.max(0, total - span)
    return [top, top + span]
  }

  maxCount(): number {
    return Math.max(1, ...this.windows.map((w) => w.count ?? 0))
  }

  // --- Scrolling (shared by all windows) ---

  /** Marks the view as moving; it settles 150 ms after the last call. */
  touch() {
    this.interacting = true
    clearTimeout(this.interactTimer)
    this.interactTimer = setTimeout(() => (this.interacting = false), 150)
  }

  /** Scrolls by `dy` CSS pixels of window `w` (default: the largest), `h` px tall. */
  scrollRows(dy: number, h: number, w?: Win) {
    const ref = w ?? this.windows.reduce<Win | undefined>((a, b) => ((b.count ?? 0) > (a?.count ?? -1) ? b : a), undefined)
    if (!ref) return
    const { span, total } = this.extent(ref)
    const range = total - span
    if (range <= 0 || h <= 0) return
    this.touch()
    this.scrollTo(this.vertical.pos + (dy * span) / h / range)
  }

  scrollTo(pos: number) {
    this.vertical = { ...this.vertical, pos: Math.max(0, Math.min(1, pos)) }
  }

  setVerticalMode(mode: VerticalMode) {
    if (mode !== this.vertical.mode) this.vertical = { ...this.vertical, mode }
  }
}

export const app = new AppState()
