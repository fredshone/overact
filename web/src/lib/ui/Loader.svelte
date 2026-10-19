<script lang="ts">
  import { app, type Preset } from '../state.svelte'
  import type { Inspection, SortKey, TableMapping } from '../engine/types'
  import { count, parseClock } from '../format'
  import { fileKind, guessAttrId, guessRoles, looksLikeSchedules, mappingProblems, type SchedRoles } from '../roles'
  import TableCell from './TableCell.svelte'

  interface Slot {
    file: File | null
    info: Inspection | null
    error: string
    busy: boolean
  }
  const empty = (): Slot => ({ file: null, info: null, error: '', busy: false })

  let sched = $state<Slot>(empty())
  let attrs = $state<Slot>(empty())
  let roles = $state<SchedRoles>({ id: null, act: null, start: null, end: null, duration: null, seq: null })
  let useDuration = $state(false)
  let attrId = $state<string | null>(null)
  let excluded = $state<string[]>([])
  let axisStart = $state('00:00')
  let axisEnd = $state('24:00')
  let unit = $state<TableMapping['time_unit']>('minute')
  let fraction = $state(1)
  let dragging = $state(false)
  let url = $state('')
  let overactInput = $state<HTMLInputElement>()
  let notice = $state('')

  // Absolute, since the worker would resolve a relative base (`./`) against /assets/.
  const demo = (f: string) => new URL(`${import.meta.env.BASE_URL}demo/${f}`, document.baseURI).href
  // Both demos open sorted by the duration of each person's first episode.
  const demoSort: SortKey[] = [{ col: { feature: 'measure', measure: 'duration', activity: null, not: false, nth: 1 }, desc: false }]
  const byEmployment = (title: string, ...categories: string[]) => ({
    title,
    filter: [{ col: { column: 'employment' }, negate: false, type: 'in' as const, values: [], categories }],
  })
  const demos: { label: string; title: string; url: string; preset: Preset }[] = [
    { label: 'Demo dataset', title: 'Synthetic population, one window', url: demo('demo.overact'), preset: { sort: demoSort } },
    {
      label: 'Small demo',
      title: '200 synthetic persons, compared by employment in four windows',
      url: demo('demo-small.overact'),
      preset: {
        sort: demoSort,
        windows: [
          byEmployment('Employed', 'ft-employed', 'pt-employed'),
          byEmployment('Student', 'student'),
          byEmployment('Retired', 'retired'),
          byEmployment('Unemployed', 'unemployed'),
        ],
      },
    },
  ]
  const schedCols = $derived(sched.info?.columns.map((c) => c.name) ?? [])
  const attrCols = $derived(attrs.info?.columns.map((c) => c.name) ?? [])
  const axis = $derived<[number, number]>([parseClock(axisStart, 0) ?? NaN, parseClock(axisEnd, 0) ?? NaN])

  const mapping = $derived<TableMapping>({
    id_col: roles.id ?? '',
    act_col: roles.act ?? '',
    start_col: roles.start ?? '',
    end_col: useDuration ? null : roles.end,
    duration_col: useDuration ? roles.duration : null,
    seq_col: roles.seq,
    attr_id_col: attrs.info ? attrId : null,
    axis,
    time_unit: unit,
    exclude: attrs.info ? excluded : [],
  })
  const problems = $derived(mappingProblems(mapping, sched.info))
  const ready = $derived(problems.length === 0 && !sched.busy && !attrs.busy && !sched.error && !attrs.error)

  // --- remembered mappings (per set of column names; a convenience only) ---
  const MAP_KEY = 'overact.mappings'
  type Stored = { roles: SchedRoles; useDuration: boolean; axis: [string, string]; unit: TableMapping['time_unit'] }
  function stored(cols: string[]): Stored | null {
    try {
      return JSON.parse(localStorage.getItem(MAP_KEY) ?? '{}')[cols.join('\u0001')] ?? null
    } catch {
      return null
    }
  }
  function remember() {
    try {
      const all = JSON.parse(localStorage.getItem(MAP_KEY) ?? '{}')
      all[schedCols.join('\u0001')] = { roles, useDuration, axis: [axisStart, axisEnd], unit } satisfies Stored
      const keys = Object.keys(all)
      for (const k of keys.slice(0, Math.max(0, keys.length - 20))) delete all[k]
      localStorage.setItem(MAP_KEY, JSON.stringify(all))
    } catch {
      /* storage unavailable */
    }
  }

  async function inspect(slot: Slot, f: File): Promise<Inspection | null> {
    slot.file = f
    slot.info = null
    slot.error = ''
    slot.busy = true
    try {
      const info = await app.inspect(f)
      if (slot.file !== f) return null
      slot.info = info
      return info
    } catch (e) {
      if (slot.file === f) slot.error = e instanceof Error ? e.message : String(e)
      return null
    } finally {
      if (slot.file === f) slot.busy = false
    }
  }

  async function setSchedules(f: File | null) {
    if (!f) {
      sched = empty()
      return
    }
    const info = await inspect(sched, f)
    if (!info) return
    const cols = info.columns.map((c) => c.name)
    const s = stored(cols)
    if (s) {
      roles = s.roles
      useDuration = s.useDuration
      ;[axisStart, axisEnd] = s.axis
      unit = s.unit
    } else {
      roles = guessRoles(cols)
      useDuration = !roles.end && !!roles.duration
    }
    if (attrs.info) attrId = guessAttrId(attrCols, roles.id)
  }

  async function setAttributes(f: File | null) {
    if (!f) {
      attrs = empty()
      return
    }
    const info = await inspect(attrs, f)
    if (!info) return
    attrId = guessAttrId(info.columns.map((c) => c.name), roles.id)
    excluded = []
  }

  /** Files dropped on the page: an .overact opens directly; tables go to the cell they look like. */
  async function addFiles(files: File[]) {
    notice = ''
    const ov = files.find((f) => fileKind(f.name) === 'overact')
    if (ov) return openOveract(ov)
    const tables = files.filter((f) => fileKind(f.name) === 'table').slice(0, 2)
    if (!tables.length) {
      if (files.length) notice = 'Drop .csv, .tsv, .parquet or .overact files.'
      return
    }
    if (tables.length === 1 && !sched.file) return setSchedules(tables[0])
    if (tables.length === 1) return setAttributes(tables[0])
    const infos = await Promise.all(tables.map((f) => app.inspect(f).catch(() => null)))
    const isSched = infos.map((i) => !!i && looksLikeSchedules(i.columns.map((c) => c.name)))
    const k = isSched[1] && !isSched[0] ? 1 : 0
    await Promise.all([setSchedules(tables[k]), setAttributes(tables[1 - k])])
  }

  function load() {
    if (!ready || !sched.file) return
    remember()
    app.openTables({ schedules: sched.file, attributes: attrs.file }, $state.snapshot(mapping) as TableMapping, { fraction, seed: 1 })
  }

  function openOveract(f: File | undefined) {
    if (f) app.open({ kind: 'file', file: f }, f.name, { fraction, seed: 1 })
  }

  function openUrl(u: string, preset: Preset = {}) {
    if (u) app.open({ kind: 'url', url: u }, u.split('/').pop() ?? u, { fraction, seed: 1 }, preset)
  }

  function toggleColumn(c: string, on: boolean) {
    excluded = on ? excluded.filter((x) => x !== c) : [...excluded, c]
  }

  const attrChoices = $derived(attrCols.filter((c) => c !== attrId))
  const highlightSched = $derived(
    [roles.id, roles.act, roles.start, useDuration ? roles.duration : roles.end, roles.seq].filter((x): x is string => !!x),
  )
  const typeOf = (info: Inspection | null, name: string) => info?.columns.find((c) => c.name === name)?.type
</script>

{#snippet colSelect(label: string, value: string | null, set: (v: string | null) => void, optional = false)}
  <label class="field">
    <span>{label}</span>
    <select value={value ?? ''} onchange={(e) => set(e.currentTarget.value || null)}>
      <option value="">{optional ? '(none)' : '— choose —'}</option>
      {#each schedCols as c (c)}<option value={c}>{c}{typeOf(sched.info, c) === 'text' ? '' : ` (${typeOf(sched.info, c)})`}</option>{/each}
    </select>
  </label>
{/snippet}

<div
  class="loader"
  class:dragging
  role="region"
  aria-label="Open a dataset"
  ondragover={(e) => {
    e.preventDefault()
    dragging = true
  }}
  ondragleave={() => (dragging = false)}
  ondrop={(e) => {
    e.preventDefault()
    dragging = false
    addFiles([...(e.dataTransfer?.files ?? [])])
  }}
>
  <div class="card">
    <div class="intro">
      {#if app.adding}
        <div class="add-head">
          <h1>Add a dataset</h1>
          <button onclick={() => app.cancelAdding()}>Cancel</button>
        </div>
        <p class="muted">
          Load another dataset to compare with {app.datasets.map((d) => d.name).join(', ')}. Windows can then show either; it must have the same time axis.
        </p>
      {:else}
        <h1>overact</h1>
        <p class="muted">
          View and compare human activity schedules. Files stay on your computer: nothing is uploaded.
        </p>
      {/if}
    </div>

    {#if app.status === 'loading'}
      <div class="progress">
        <div>Loading {app.sourceName}: {app.progress.phase}</div>
        <progress value={app.progress.done} max={app.progress.total || 1}></progress>
      </div>
    {:else}
      <div class="cells">
        <TableCell
          title="Schedules"
          note="required · one row per activity"
          file={sched.file}
          info={sched.info}
          error={sched.error}
          busy={sched.busy}
          highlight={highlightSched}
          onfile={setSchedules}
        >
          <div class="grid">
            {@render colSelect('Person id', roles.id, (v) => (roles.id = v))}
            {@render colSelect('Activity', roles.act, (v) => (roles.act = v))}
            {@render colSelect('Start', roles.start, (v) => (roles.start = v))}
            <label class="field">
              <span>
                <select class="inline" bind:value={useDuration} aria-label="End or duration">
                  <option value={false}>End</option>
                  <option value={true}>Duration</option>
                </select>
              </span>
              <select
                value={(useDuration ? roles.duration : roles.end) ?? ''}
                onchange={(e) => {
                  const v = e.currentTarget.value || null
                  if (useDuration) roles.duration = v
                  else roles.end = v
                }}
              >
                <option value="">— choose —</option>
                {#each schedCols as c (c)}<option value={c}>{c}</option>{/each}
              </select>
            </label>
            {@render colSelect('Order (ties)', roles.seq, (v) => (roles.seq = v), true)}
          </div>
        </TableCell>

        <TableCell
          title="Attributes"
          note="optional · one row per person"
          file={attrs.file}
          info={attrs.info}
          error={attrs.error}
          busy={attrs.busy}
          highlight={attrId ? [attrId] : []}
          onfile={setAttributes}
        >
          <div class="grid">
            <label class="field">
              <span>Person id</span>
              <select bind:value={attrId}>
                {#each attrCols as c (c)}<option value={c}>{c}</option>{/each}
              </select>
            </label>
          </div>
          <div class="cols">
            <div class="colhead">
              <span>Columns ({attrChoices.length - excluded.filter((c) => attrChoices.includes(c)).length} of {attrChoices.length})</span>
              <button class="link" onclick={() => (excluded = [])}>all</button>
              <button class="link" onclick={() => (excluded = [...attrChoices])}>none</button>
            </div>
            <div class="collist">
              {#each attrChoices as c (c)}
                <label title={typeOf(attrs.info, c)}>
                  <input type="checkbox" checked={!excluded.includes(c)} onchange={(e) => toggleColumn(c, e.currentTarget.checked)} />
                  {c}
                </label>
              {/each}
            </div>
            <p class="muted small">Columns with mostly distinct text values (ids) are skipped automatically.</p>
          </div>
        </TableCell>
      </div>

      <div class="options">
        <label class="field">
          <span>Day from</span>
          <input class="time" bind:value={axisStart} aria-label="Time axis start" />
        </label>
        <label class="field">
          <span>to</span>
          <input class="time" bind:value={axisEnd} aria-label="Time axis end" />
        </label>
        <label class="field">
          <span>Times are in</span>
          <select bind:value={unit}>
            <option value="minute">minutes</option>
            <option value="second">seconds</option>
            <option value="hour">hours</option>
          </select>
        </label>
        <label class="field">
          <span>Load</span>
          <select bind:value={fraction}>
            <option value={1}>all persons</option>
            <option value={0.5}>a random 50%</option>
            <option value={0.25}>a random 25%</option>
            <option value={0.1}>a random 10%</option>
            <option value={0.01}>a random 1%</option>
          </select>
        </label>
        <span class="spacer"></span>
        <button class="primary load" disabled={!ready} onclick={load} title={problems.join(' ')}>
          Load{sched.info?.n_rows ? ` ${count(sched.info.n_rows)} activities` : ''}
        </button>
      </div>
      {#if sched.info && problems.length}<p class="muted small">{problems.join(' ')}</p>{/if}
      <p class="muted small">
        Times are measured from midnight; an axis like 04:00 to 28:00 shows a day that starts at 4 am. Gaps are filled with
        <em>unknown</em>, overlaps trimmed; the dataset summary lists every fix.
      </p>

      {#if app.status === 'error'}
        <p class="error">Could not open {app.sourceName}: {app.error}</p>
      {/if}
      {#if notice}<p class="error">{notice}</p>{/if}

      <div class="packed">
        <span class="muted">Already packed?</span>
        <button onclick={() => overactInput?.click()}>Open .overact file…</button>
        <input bind:this={overactInput} type="file" accept=".overact" hidden onchange={(e) => openOveract(e.currentTarget.files?.[0])} />
        {#each demos as d (d.url)}<button title={d.title} onclick={() => openUrl(d.url, d.preset)}>{d.label}</button>{/each}
        <form
          class="url"
          onsubmit={(e) => {
            e.preventDefault()
            openUrl(url)
          }}
        >
          <input type="url" placeholder="https://… .overact (needs CORS + Range)" bind:value={url} />
          <button type="submit" disabled={!url}>Open URL</button>
        </form>
      </div>
    {/if}
  </div>
</div>

<style>
  .add-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .loader {
    height: 100%;
    overflow: auto;
    display: grid;
    place-items: center;
    padding: 16px;
  }
  .loader.dragging {
    background: color-mix(in srgb, var(--accent) 10%, var(--bg));
  }
  .card {
    width: min(980px, 100%);
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 12px;
    padding: 24px;
    display: grid;
    gap: 14px;
  }
  .intro {
    display: grid;
    gap: 4px;
  }
  h1 {
    margin: 0;
    font-size: 26px;
    letter-spacing: -0.02em;
  }
  p {
    margin: 0;
  }
  .cells {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 12px;
    align-items: start;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 8px;
  }
  .field {
    display: grid;
    gap: 3px;
    font-size: 12px;
  }
  .field > span {
    color: var(--muted);
    min-height: 22px;
    display: flex;
    align-items: end;
  }
  .field select,
  .field input {
    width: 100%;
    min-width: 0;
  }
  select.inline {
    width: auto;
    padding: 0 2px;
    font-size: 12px;
    color: var(--muted);
    background: transparent;
    border: 0;
  }
  .cols {
    display: grid;
    gap: 4px;
  }
  .colhead {
    display: flex;
    gap: 8px;
    font-size: 12px;
    color: var(--muted);
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font-size: 12px;
  }
  .collist {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    grid-auto-rows: max-content;
    gap: 2px 8px;
    max-height: 140px;
    overflow: auto;
    padding: 6px 8px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
    font-size: 12px;
  }
  .collist label {
    display: flex;
    gap: 4px;
    align-items: center;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .options {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    align-items: end;
  }
  .options .field {
    width: auto;
  }
  .time {
    width: 70px !important;
  }
  .spacer {
    flex: 1;
  }
  .load {
    padding: 8px 18px;
    font-size: 14px;
  }
  .packed {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    border-top: 1px solid var(--line);
    padding-top: 12px;
  }
  .url {
    display: flex;
    gap: 8px;
    flex: 1;
    min-width: 240px;
  }
  .url input {
    flex: 1;
    min-width: 0;
  }
  .error {
    color: var(--danger);
  }
  .small {
    font-size: 12px;
  }
  progress {
    width: 100%;
  }
  .progress {
    display: grid;
    gap: 6px;
  }
  @media (max-width: 720px) {
    .cells {
      grid-template-columns: minmax(0, 1fr);
    }
    .card {
      padding: 16px;
    }
  }
</style>
