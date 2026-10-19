<script lang="ts">
  import { untrack } from 'svelte'
  import { outside } from './outside'
  import type { ColRef, Condition, Feature, Header } from '../engine/types'
  import { clock, colKind, colName, isDurationFeature, isFeature, isTimeFeature, number, parseClock } from '../format'

  let {
    header,
    initial,
    col: initialCol,
    onapply,
    onclose,
  }: {
    /** Header of the window's dataset (columns and categories). */
    header: Header
    initial: Condition | null
    col: ColRef
    onapply: (c: Condition) => void
    onclose: () => void
  } = $props()

  const h = untrack(() => header)
  // The builder is created fresh for each edit, so it reads its props once.
  const init = untrack(() => ({ cond: initial ? structuredClone($state.snapshot(initial)) as Condition : null, col: structuredClone($state.snapshot(initialCol)) as ColRef }))
  const initFeat = isFeature(init.cond?.col ?? init.col) ? ((init.cond?.col ?? init.col) as Feature) : null
  let col = $state<ColRef>(init.cond?.col ?? init.col)
  let negate = $state(init.cond?.negate ?? false)
  const kind = $derived(colKind(h, col))
  const meta = $derived(isFeature(col) ? null : h.columns[col.column])
  const feat = $derived(isFeature(col) ? (col as Feature) : null)
  const timeLike = $derived(feat ? isTimeFeature(feat) : false)
  const axis0 = h.time_axis.start

  // Numeric range state (as text so empty = unbounded).
  let minText = $state('')
  let maxText = $state('')
  // Category / boolean selection.
  let selected = $state<Set<number>>(new Set())
  let includeNull = $state(false)
  let catQuery = $state('')

  function fmtTime(m: number) {
    return clock(axis0 + m)
  }
  function parseTime(s: string): number | null {
    return parseClock(s, axis0)
  }
  /** A bound: null when empty (no limit), undefined when not a valid number or time. */
  function parseNum(s: string): number | null | undefined {
    const t = s.trim()
    if (!t) return null
    const n = timeLike ? parseTime(t) : Number(t)
    return n !== null && Number.isFinite(n) ? n : undefined
  }
  const minBad = $derived(parseNum(minText) === undefined)
  const maxBad = $derived(parseNum(maxText) === undefined)

  // Initialise from an existing condition.
  const ic = init.cond
  if (ic) {
    if (ic.type === 'range') {
      const tl = initFeat ? isTimeFeature(initFeat) : false
      const f = (v: number | null) => (v === null ? '' : tl ? fmtTime(v) : String(v))
      minText = f(ic.min)
      maxText = f(ic.max)
    } else if (ic.type === 'in') {
      selected = new Set(ic.values)
      includeNull = !!ic.null
    }
  } else if (colKind(h, init.col) === 'boolean') {
    selected = new Set([1])
  }

  // Time window controls for doing_at / doing_during.
  const f0 = initFeat
  let tStart = $state(f0 && 'start' in f0 ? fmtTime(f0.start) : f0 && 'minute' in f0 ? fmtTime(f0.minute) : '08:00')
  let tEnd = $state(f0 && 'end' in f0 ? fmtTime(f0.end) : '09:00')
  let mode = $state<'at' | 'any' | 'all'>(
    f0?.feature === 'doing_at' ? 'at' : f0?.feature === 'doing_during' && f0.all ? 'all' : 'any',
  )
  let activity = $state(f0 && 'activity' in f0 ? (f0.activity ?? 0) : 0)
  const scheduleTime = $derived(feat?.feature === 'doing_at' || feat?.feature === 'doing_during')

  function scheduleCol(): ColRef | null {
    const s = parseTime(tStart)
    if (s === null) return null
    if (mode === 'at') return { feature: 'doing_at', activity, minute: Math.max(0, s) }
    const e = parseTime(tEnd)
    if (e === null || e <= s) return null
    return { feature: 'doing_during', activity, start: Math.max(0, s), end: e, all: mode === 'all' }
  }

  const cats = $derived.by(() => {
    const all = (meta?.categories ?? []).map((c, i) => ({ c, i }))
    const q = catQuery.trim().toLowerCase()
    return q ? all.filter((x) => x.c.toLowerCase().includes(q)) : all
  })

  function build(): Condition | null {
    if (scheduleTime) {
      const c = scheduleCol()
      if (!c) return null
      return { col: c, negate, type: 'in', values: [1] }
    }
    if (kind === 'numeric') {
      const min = parseNum(minText)
      const max = parseNum(maxText)
      if (min === undefined || max === undefined) return null
      return { col: $state.snapshot(col) as ColRef, negate, type: 'range', min, max }
    }
    return { col: $state.snapshot(col) as ColRef, negate, type: 'in', values: [...selected].sort((a, b) => a - b), null: includeNull }
  }

  const preview = $derived(build())

  function apply() {
    const c = build()
    if (c) onapply(c)
  }

  function toggle(i: number) {
    const s = new Set(selected)
    if (s.has(i)) s.delete(i)
    else s.add(i)
    selected = s
  }

  const rangeHint = $derived.by(() => {
    if (!meta || meta.min === null || meta.max === null) return ''
    return `${number(meta.min)} – ${number(meta.max)}`
  })
</script>

<div
  class="popover builder"
  use:outside={onclose}
  role="dialog"
  aria-label="Edit filter"
  tabindex="-1"
  onkeydown={(e) => {
    if (e.key === 'Escape') onclose()
    if (e.key === 'Enter' && (e.target as HTMLElement).tagName === 'INPUT') apply()
  }}
>
  <div class="title">{scheduleTime ? 'Schedule condition' : colName(h, col)}</div>

  {#if scheduleTime}
    <div class="grid">
      <label for="fb-act">Activity</label>
      <select id="fb-act" bind:value={activity}>
        {#each h.activities as a (a.code)}<option value={a.code}>{a.label}</option>{/each}
      </select>
      <label for="fb-mode">When</label>
      <select id="fb-mode" bind:value={mode}>
        <option value="at">at a time</option>
        <option value="any">at any time during</option>
        <option value="all">for all of</option>
      </select>
      <label for="fb-t0">{mode === 'at' ? 'Time' : 'From'}</label>
      <input id="fb-t0" bind:value={tStart} placeholder="HH:MM" />
      {#if mode !== 'at'}
        <label for="fb-t1">To</label>
        <input id="fb-t1" bind:value={tEnd} placeholder="HH:MM" />
      {/if}
    </div>
  {:else if kind === 'numeric'}
    <div class="grid">
      <label for="fb-min">Min</label>
      <input id="fb-min" bind:value={minText} placeholder={timeLike ? 'HH:MM' : 'no limit'} aria-invalid={minBad} class:bad={minBad} />
      <label for="fb-max">Max</label>
      <input id="fb-max" bind:value={maxText} placeholder={timeLike ? 'HH:MM' : 'no limit'} aria-invalid={maxBad} class:bad={maxBad} />
    </div>
    {#if minBad || maxBad}<div class="error small">Enter {timeLike ? 'a time like 08:30' : 'a number'}, or leave it empty for no limit.</div>{/if}
    {#if rangeHint}<div class="muted small">Values range {rangeHint}. Bounds are inclusive; missing values never match.</div>{/if}
    {#if feat && isDurationFeature(feat)}<div class="muted small">Minutes.{feat.feature === 'measure' && !feat.nth ? ' Use 0 to 0 for "never".' : ''}</div>{/if}
  {:else if kind === 'boolean'}
    <div class="checks">
      <label><input type="checkbox" checked={selected.has(1)} onchange={() => toggle(1)} /> true</label>
      <label><input type="checkbox" checked={selected.has(0)} onchange={() => toggle(0)} /> false</label>
      <label><input type="checkbox" bind:checked={includeNull} /> missing</label>
    </div>
  {:else}
    {#if (meta?.categories?.length ?? 0) > 12}
      <input placeholder="Search values…" bind:value={catQuery} />
    {/if}
    <div class="checks cats">
      {#each cats as { c, i } (i)}
        <label><input type="checkbox" checked={selected.has(i)} onchange={() => toggle(i)} /> {c}</label>
      {/each}
      <label class="muted"><input type="checkbox" bind:checked={includeNull} /> missing</label>
    </div>
    <div class="row small">
      <button class="ghost" onclick={() => (selected = new Set(meta?.categories?.map((_, i) => i)))}>all</button>
      <button class="ghost" onclick={() => (selected = new Set())}>none</button>
    </div>
  {/if}

  <label class="neg"><input type="checkbox" bind:checked={negate} /> Exclude matches (NOT)</label>

  <div class="row end">
    <button class="ghost" onclick={onclose}>Cancel</button>
    <button class="primary" disabled={!preview} onclick={apply}>{init.cond ? 'Update' : 'Add filter'}</button>
  </div>
</div>

<style>
  .builder {
    width: 300px;
    display: grid;
    gap: 8px;
  }
  .title {
    font-weight: 600;
  }
  .grid {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 8px;
    align-items: center;
  }
  .checks {
    display: grid;
    gap: 2px;
  }
  .cats {
    max-height: 220px;
    overflow: auto;
  }
  .row {
    display: flex;
    gap: 6px;
  }
  .end {
    justify-content: flex-end;
  }
  .neg {
    font-size: 12px;
  }
  .bad {
    border-color: var(--danger);
  }
  .error {
    color: var(--danger);
  }
</style>
