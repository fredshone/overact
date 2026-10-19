<script lang="ts">
  import { onMount, untrack } from 'svelte'
  import { outside } from './outside'
  import { app } from '../state.svelte'
  import type { ColRef, Condition, Feature, Header, Measure } from '../engine/types'
  import { colName, normalizeFeature, ordinal, sameCol } from '../format'
  import ScheduleFilter from './ScheduleFilter.svelte'

  let {
    header = null,
    purpose,
    onpick,
    onclose,
    onconditions,
    initial = null,
  }: {
    /** Columns to pick from (default: the union of the datasets). */
    header?: Header | null
    purpose: 'filter' | 'sort'
    onpick: (c: ColRef) => void
    onclose: () => void
    /** Filters only: finished conditions from the schedule builder (all must hold). */
    onconditions?: (cs: Condition[]) => void
    /** A column being edited: preselected in the list or the measure builder. */
    initial?: ColRef | null
  } = $props()
  const H = $derived(header ?? app.header!)
  const init = untrack(() => (initial ? normalizeCol($state.snapshot(initial) as ColRef) : null))

  function normalizeCol(c: ColRef): ColRef {
    return 'feature' in c ? normalizeFeature(c) : c
  }

  let query = $state('')
  let input: HTMLInputElement

  interface Entry {
    col: ColRef
    label: string
    group: string
    note?: string
  }

  const entries = $derived.by<Entry[]>(() => {
    const h = H
    const out: Entry[] = []
    h.columns.forEach((c, i) => {
      const empty = c.min === null && (c.nullCount ?? 0) > 0
      const constant = !empty && c.min === c.max
      out.push({
        col: { column: i },
        label: c.name,
        group: 'Attributes',
        note: empty ? 'empty' : constant ? 'constant' : c.kind === 'categorical' ? `${c.categories?.length} values` : c.kind,
      })
    })
    return out
  })

  const shown = $derived.by(() => {
    const q = query.trim().toLowerCase()
    return q ? entries.filter((e) => e.label.toLowerCase().includes(q)) : entries
  })

  // The edited column starts highlighted (when listed and not searching).
  const startIndex = $derived(init ? Math.max(0, entries.findIndex((e) => sameCol(e.col, init))) : 0)
  let active = $state(0)
  $effect(() => {
    void shown
    active = query.trim() ? 0 : untrack(() => startIndex)
  })

  // --- Schedule measure: (i) what, (ii) which episode, (iii) of which activity ---
  const MEASURES: { m: Measure; label: string }[] = [
    { m: 'count', label: 'Count' },
    { m: 'start', label: 'Start' },
    { m: 'duration', label: 'Duration' },
    { m: 'end', label: 'End' },
  ]
  const POSITIONS = [1, 2, 3, 4, 5, 6, 7, 8, -1, -2, -3]
  const initMeasure = init && 'feature' in init && init.feature === 'measure' ? init : null
  const initDistinct = init !== null && 'feature' in init && init.feature === 'distinct'
  let measure = $state<Measure>(initDistinct ? 'count' : (initMeasure?.measure ?? 'duration'))
  let nth = $state(initMeasure?.nth ?? 0)
  // 'any', 'a:<code>' or 'n:<code>' (anything but)
  let act = $state(
    initMeasure && initMeasure.activity !== null ? `${initMeasure.not ? 'n' : 'a'}:${initMeasure.activity}` : 'any',
  )
  // Count episodes, or count different activities (the "distinct" feature).
  let countWhat = $state<'episodes' | 'distinct'>(initDistinct ? 'distinct' : 'episodes')

  function pickMeasure(m: Measure) {
    measure = m
    // Start/end always pick one episode; count always counts all of them.
    if (m === 'count') nth = 0
    else if (m === 'start' && nth === 0) nth = 1
    else if (m === 'end' && nth === 0) nth = -1
  }

  const built = $derived.by<Feature>(() => {
    if (measure === 'count' && countWhat === 'distinct') return { feature: 'distinct' }
    const [kind, code] = act.split(':')
    return normalizeFeature({
      feature: 'measure',
      measure,
      activity: kind === 'any' ? null : Number(code),
      not: kind === 'n',
      nth,
    })
  })

  onMount(() => {
    input.focus()
  })

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') onclose()
    else if (e.key === 'ArrowDown') {
      active = Math.min(shown.length - 1, active + 1)
      e.preventDefault()
    } else if (e.key === 'ArrowUp') {
      active = Math.max(0, active - 1)
      e.preventDefault()
    } else if (e.key === 'Enter' && shown[active]) onpick(shown[active].col)
  }
</script>

<div class="popover picker" use:outside={onclose} role="dialog" aria-label="Choose a column">
  <input bind:this={input} bind:value={query} placeholder="Search attributes…" onkeydown={onKey} />
  <div class="list" class:short={purpose === 'filter'} role="listbox">
    {#each shown as e, i (i)}
      {#if i === 0 || shown[i - 1].group !== e.group}
        <div class="group">{e.group}</div>
      {/if}
      <button
        class="item"
        class:active={i === active}
        role="option"
        aria-selected={i === active}
        onmouseenter={() => (active = i)}
        onclick={() => onpick(e.col)}
      >
        <span>{e.label}</span>
        {#if e.note}<span class="muted note">{e.note}</span>{/if}
      </button>
    {/each}
    {#if !shown.length}<div class="muted none">No matching attributes</div>{/if}
  </div>
  {#if purpose === 'filter' && onconditions}
    <ScheduleFilter header={H} {onconditions} />
  {:else}
  <div class="builder" role="group" aria-label="Schedule measure">
    <div class="group">Schedule measure</div>
    <div class="seg" role="radiogroup" aria-label="Measure">
      {#each MEASURES as { m, label } (m)}
        <button role="radio" aria-checked={measure === m} class:on={measure === m} onclick={() => pickMeasure(m)}>{label}</button>
      {/each}
    </div>
    <div class="sentence">
      {#if measure === 'count'}
        <select bind:value={countWhat} aria-label="What to count">
          <option value="episodes">episodes of</option>
          <option value="distinct">different activities</option>
        </select>
      {:else}
        <span class="muted">of</span>
        <select bind:value={nth} aria-label="Which episode">
          {#if measure === 'duration'}<option value={0}>all episodes (total)</option>{/if}
          {#each POSITIONS as k (k)}<option value={k}>the {ordinal(k)} episode</option>{/each}
        </select>
        <span class="muted">of</span>
      {/if}
      {#if !(measure === 'count' && countWhat === 'distinct')}
      <select bind:value={act} aria-label="Which activity">
        <option value="any">any activity</option>
        <optgroup label="Only">
          {#each H.activities as a (a.code)}<option value="a:{a.code}">{a.label}</option>{/each}
        </optgroup>
        <optgroup label="Anything except">
          {#each H.activities as a (a.code)}<option value="n:{a.code}">not {a.label}</option>{/each}
        </optgroup>
      </select>
      {/if}
    </div>
    <div class="use">
      <span class="preview" title="Episodes are the segments of a schedule, in time order">{colName(H, built)}</span>
      <button class="primary" onclick={() => onpick(built)}>{initial ? 'Update' : purpose === 'sort' ? 'Sort' : 'Filter'}</button>
    </div>
  </div>
  {/if}
</div>

<style>
  .picker {
    width: 320px;
    display: grid;
    gap: 6px;
  }
  .list {
    max-height: 260px;
    overflow: auto;
    display: grid;
  }
  .list.short {
    max-height: 190px;
  }
  .group {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted);
    padding: 6px 6px 2px;
  }
  .item {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    text-align: left;
    background: transparent;
    padding: 4px 6px;
  }
  .item.active {
    background: var(--chip);
  }
  .note {
    font-size: 11px;
  }
  .none {
    padding: 6px;
  }
  .builder {
    display: grid;
    gap: 6px;
    border-top: 1px solid var(--line);
    padding-top: 2px;
  }
  .builder .group {
    padding-left: 0;
  }
  .seg {
    display: flex;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .seg button {
    flex: 1;
    border-radius: 0;
    background: transparent;
    padding: 3px 4px;
  }
  .seg button.on {
    background: var(--chip-hover);
    font-weight: 600;
  }
  .sentence {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
  }
  .sentence select {
    min-width: 0;
    max-width: 100%;
  }
  .use {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .preview {
    font-size: 12px;
    font-style: italic;
  }
</style>
