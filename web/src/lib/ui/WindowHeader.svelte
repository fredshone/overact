<script lang="ts">
  import { untrack } from 'svelte'
  import { app, MAX_WINDOWS, type Win } from '../state.svelte'
  import type { ColRef, Condition } from '../engine/types'
  import { count, describeCondition, pct } from '../format'
  import ColumnPicker from './ColumnPicker.svelte'
  import { exportIds, exportPNG } from '../export'
  import FilterBuilder from './FilterBuilder.svelte'
  import Icon from './Icon.svelte'

  let { win }: { win: Win } = $props()

  // A window's dataset never changes (showing another replaces the window).
  const ds = untrack(() => app.datasetOf(win))
  const h = ds.header
  const nPersons = h.n_persons

  type Pop =
    | { kind: 'filterCol' }
    | { kind: 'filter'; col: ColRef; index: number | null }
    | { kind: 'menu' }
    | null
  let pop = $state<Pop>(null)

  function snapshotFilter(): Condition[] {
    return $state.snapshot(win.filter) as Condition[]
  }

  function applyFilter(c: Condition, index: number | null) {
    const f = snapshotFilter()
    if (index === null) f.push(c)
    else f[index] = c
    app.setFilter(win, f)
    pop = null
  }

  function removeFilter(i: number) {
    const f = snapshotFilter()
    f.splice(i, 1)
    app.setFilter(win, f)
  }

  // Drag the grip to move the window. Listeners are on the document because
  // the window's element moves in the DOM while dragging.
  function onGripDown(e: PointerEvent) {
    if (e.button !== 0 || app.windows.length < 2) return
    e.preventDefault()
    app.draggingWin = win.id
    const move = (ev: PointerEvent) => {
      const els = [...document.querySelectorAll<HTMLElement>('.windows > .window')]
      let best = -1
      let bestD = Infinity
      els.forEach((el, i) => {
        const r = el.getBoundingClientRect()
        const d = Math.hypot(ev.clientX - (r.left + r.width / 2), ev.clientY - (r.top + r.height / 2))
        if (d < bestD) {
          bestD = d
          best = i
        }
      })
      if (best >= 0) app.moveWindow(win.id, best)
    }
    const up = () => {
      app.draggingWin = null
      document.removeEventListener('pointermove', move)
      document.removeEventListener('pointerup', up)
      document.removeEventListener('pointercancel', up)
    }
    document.addEventListener('pointermove', move)
    document.addEventListener('pointerup', up)
    document.addEventListener('pointercancel', up)
  }

  function onGripKey(e: KeyboardEvent) {
    const d = e.key === 'ArrowLeft' ? -1 : e.key === 'ArrowRight' ? 1 : 0
    if (!d) return
    e.preventDefault()
    e.stopPropagation()
    app.moveWindow(win.id, app.windows.findIndex((w) => w.id === win.id) + d)
  }

  const others = $derived(app.windows.filter((w) => w.id !== win.id))
  const share = $derived(win.matched !== null && nPersons ? win.matched / nPersons : null)
  const sampled = $derived(win.count !== null && win.matched !== null && win.count < win.matched)
  const countTitle = $derived(
    (sampled
      ? `${win.count?.toLocaleString()} sampled from ${win.matched?.toLocaleString()} matching, of ${nPersons.toLocaleString()} persons in ${ds.name}`
      : `${win.count?.toLocaleString() ?? '…'} of ${nPersons.toLocaleString()} persons in ${ds.name}`) +
      (win.jobMs ? ` · computed in ${win.jobMs.toFixed(0)} ms` : ''),
  )
</script>

<header class="head">
  <div class="top">
    {#if app.windows.length > 1}
      <button
        class="ghost grip"
        title="Drag to move this window (or focus and use ← / →)"
        aria-label="Move window"
        onpointerdown={onGripDown}
        onkeydown={onGripKey}><Icon name="grip" size={14} /></button>
    {/if}
    <input class="title" value={win.title} oninput={(e) => app.renameWindow(win.id, e.currentTarget.value)} aria-label="Window title" />
    <span class="count" title={countTitle}>
      {#if win.count !== null}
        <b>{count(win.count)}</b>
        {#if sampled}<span class="muted">of {count(win.matched ?? 0)}</span>{/if}
        <span class="muted">{pct(share ?? 0)}</span>
      {:else}
        <span class="muted">…</span>
      {/if}
    </span>
    <span class="spacer"></span>
    {#if app.datasets.length > 1}
      <select class="dataset" value={win.dataset} title="Dataset shown in this window" aria-label="Dataset" onchange={(e) => app.setWindowDataset(win, Number(e.currentTarget.value))}>
        {#each app.datasets as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
      </select>
    {/if}
    <button class="ghost icon" title="Clone window" aria-label="Clone window" disabled={app.windows.length >= MAX_WINDOWS} onclick={() => app.addWindow(win)}><Icon name="clone" /></button>
    <button class="ghost icon" title="More" aria-label="More actions" onclick={() => (pop = pop?.kind === 'menu' ? null : { kind: 'menu' })}><Icon name="more" /></button>
    {#if app.windows.length > 1}
      <button class="ghost icon" title="Close window" aria-label="Close window" onclick={() => app.removeWindow(win.id)}><Icon name="close" size={13} /></button>
    {/if}
  </div>

  <div class="chips">
    <span class="label muted">Filter</span>
    {#each win.filter as c, i (i)}
      <span class="chip filter" class:neg={c.negate}>
        <button class="ghost text" onclick={() => (pop = { kind: 'filter', col: c.col, index: i })}>{describeCondition(h, c)}</button>
        <button class="x" aria-label="Remove filter" onclick={() => removeFilter(i)}>×</button>
      </span>
    {/each}
    <button class="add" onclick={() => (pop = { kind: 'filterCol' })}>+ filter</button>
  </div>

  {#if win.busy}
    <div class="bar" style:width="{Math.max(5, win.progress * 100)}%"></div>
  {/if}

  {#if pop?.kind === 'filterCol'}
    <div class="anchor">
      <ColumnPicker
        header={h}
        purpose="filter"
        onclose={() => (pop = null)}
        onpick={(col) => (pop = { kind: 'filter', col, index: null })}
        onconditions={(cs) => {
          app.setFilter(win, [...snapshotFilter(), ...cs])
          pop = null
        }}
      />
    </div>
  {:else if pop?.kind === 'filter'}
    {@const p = pop}
    <div class="anchor">
      <FilterBuilder
        header={h}
        initial={p.index !== null ? (snapshotFilter()[p.index] ?? null) : null}
        col={p.col}
        onclose={() => (pop = null)}
        onapply={(c) => applyFilter(c, p.index)}
      />
    </div>
  {:else if pop?.kind === 'menu'}
    <div class="anchor right">
      <div class="popover menu" role="menu">
        {#each others as o (o.id)}
          <button class="ghost" role="menuitem" onclick={() => { app.copyFilter(win, o); pop = null }}>Copy filter to “{o.title}”</button>
        {/each}
        {#each app.datasets.filter((d) => d.id !== win.dataset) as d (d.id)}
          <button class="ghost" role="menuitem" disabled={app.windows.length >= MAX_WINDOWS} onclick={() => { app.addWindow(win, d.id); pop = null }}>Clone with “{d.name}”</button>
        {/each}
        <button class="ghost" role="menuitem" onclick={() => { app.setFilter(win, []); pop = null }} disabled={!win.filter.length}>Clear filter</button>
        <button class="ghost" role="menuitem" onclick={() => { pop = null; exportPNG(win) }} disabled={win.count === null}>Export this window as PNG</button>
        <button class="ghost" role="menuitem" onclick={() => { pop = null; exportIds(win, 'csv') }} disabled={win.count === null}>Export person ids (CSV)</button>
        <button class="ghost" role="menuitem" onclick={() => { pop = null; exportIds(win, 'parquet') }} disabled={win.count === null}>Export person ids (Parquet)</button>
      </div>
    </div>
  {/if}
</header>

<style>
  .head {
    position: relative;
    padding: 6px 8px 4px;
    border-bottom: 1px solid var(--line);
    display: grid;
    gap: 3px;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .grip {
    display: grid;
    place-items: center;
    padding: 3px 1px;
    margin-right: -4px;
    color: var(--muted);
    cursor: grab;
    touch-action: none;
    background: transparent;
  }
  .grip:active {
    cursor: grabbing;
  }
  .dataset {
    max-width: 140px;
    font-size: 12px;
    padding: 1px 4px;
  }
  .title {
    font-weight: 600;
    font-size: 14px;
    border-color: transparent;
    background: transparent;
    padding: 2px 4px;
    min-width: 60px;
    width: 0;
    flex: 0 1 220px;
  }
  .title:hover {
    border-color: var(--line);
  }
  .count {
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .spacer {
    flex: 1;
  }
  .icon {
    padding: 3px 5px;
    color: var(--muted);
  }
  .icon:hover {
    color: var(--fg);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    align-items: center;
    min-height: 22px;
  }
  .label {
    font-size: 11px;
    width: 42px;
  }
  .chip .text {
    padding: 0;
    background: transparent;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .chip.neg {
    background: color-mix(in srgb, var(--danger) 12%, var(--chip));
  }
  .bar {
    position: absolute;
    left: 0;
    bottom: -1px;
    height: 2px;
    background: var(--accent);
    transition: width 0.2s;
  }
  .anchor {
    left: 8px;
  }
  .anchor.right {
    left: auto;
    right: 8px;
  }
  .menu {
    display: grid;
    min-width: 240px;
  }
  .menu button {
    text-align: left;
  }
</style>
