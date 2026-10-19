<script lang="ts">
  import { flip } from 'svelte/animate'
  import { app } from '../state.svelte'
  import type { ColRef, SortKey } from '../engine/types'
  import { clock, colName, count, describeSort, formatValue, parseClock, parseCount, sameCol } from '../format'
  import ColumnPicker from './ColumnPicker.svelte'
  import Icon from './Icon.svelte'
  import { ramp } from '../palette'

  const h = $derived(app.header!)
  const axis0 = $derived(h.time_axis.start)
  /** The sort picker: adding a key (index null) or editing key `index`. */
  let picking = $state<{ index: number | null } | null>(null)

  // --- Sort (shared by all windows) ---
  function snapshotSort(): SortKey[] {
    return $state.snapshot(app.sort) as SortKey[]
  }

  function pickSort(col: ColRef) {
    const s = snapshotSort()
    const at = picking?.index ?? null
    picking = null
    if (at === null) {
      app.setSort([...s.filter((k) => !sameCol(k.col, col)), { col, desc: false }])
      return
    }
    // Replace key `at`, keeping its direction; drop any other key on that column.
    if (sameCol(s[at].col, col)) return
    const next = s.map((k, i) => (i === at ? { col, desc: k.desc } : k))
    app.setSort(next.filter((k, i) => i === at || !sameCol(k.col, col)))
  }

  // A click flips the direction, a double-click edits the key. The flip waits
  // briefly so the two clicks of a double-click don't flip it twice.
  let clickTimer: ReturnType<typeof setTimeout> | undefined
  function onChipClick(e: MouseEvent, i: number) {
    if (justDragged) return
    clearTimeout(clickTimer)
    if (e.detail > 1) return
    clickTimer = setTimeout(() => flipSort(i), 250)
  }
  function onChipDblClick(i: number) {
    if (justDragged) return
    clearTimeout(clickTimer)
    picking = { index: i }
  }

  function flipSort(i: number) {
    const s = snapshotSort()
    s[i].desc = !s[i].desc
    app.setSort(s)
  }

  function removeSort(i: number) {
    const s = snapshotSort()
    s.splice(i, 1)
    app.setSort(s)
  }

  // Drag a chip to reorder: the list previews the new order while dragging.
  // Listeners are on the document because chips move in the DOM meanwhile
  // (which would drop a pointer capture), and nothing is captured on
  // pointerdown so a plain click still reaches the direction button.
  let chipsEl: HTMLDivElement
  let order = $state<number[] | null>(null)
  let dragging = $state<number | null>(null)
  let justDragged = false
  /** Datasets lacking each sort key (their windows skip it). */
  const missing = $derived(app.sort.map((_, i) => app.sortMissing(i)))
  const missingIn = (i: number) => missing[i] ?? []
  const shown = $derived((order ?? app.sort.map((_, i) => i)).map((i) => ({ i, k: app.sort[i] })))

  function onChipDown(e: PointerEvent, from: number) {
    if (e.button !== 0 || (e.target as HTMLElement).closest('.x')) return
    // No text selection or native drag (either would cancel the pointer
    // stream mid-drag); clicks still fire.
    e.preventDefault()
    const x0 = e.clientX
    const y0 = e.clientY
    let o: number[] | null = null
    const move = (ev: PointerEvent) => {
      if (!o) {
        if (Math.hypot(ev.clientX - x0, ev.clientY - y0) < 5) return
        o = app.sort.map((_, i) => i)
        dragging = from
      }
      // Move the dragged key to the slot of the nearest chip.
      const els = [...chipsEl.querySelectorAll<HTMLElement>('.chip.sort')]
      let best = -1
      let bestD = Infinity
      els.forEach((el, pos) => {
        const r = el.getBoundingClientRect()
        const d = Math.hypot(ev.clientX - (r.left + r.width / 2), (ev.clientY - (r.top + r.height / 2)) * 3)
        if (d < bestD) {
          bestD = d
          best = pos
        }
      })
      if (best >= 0 && best !== o.indexOf(from)) {
        o = o.filter((i) => i !== from)
        o.splice(best, 0, from)
      }
      order = o
    }
    const up = () => {
      document.removeEventListener('pointermove', move)
      document.removeEventListener('pointerup', up)
      document.removeEventListener('pointercancel', up)
      dragging = null
      order = null
      if (!o) return
      justDragged = true
      setTimeout(() => (justDragged = false))
      if (o.some((i, pos) => i !== pos)) {
        const s = snapshotSort()
        app.setSort(o.map((i) => s[i]))
      }
    }
    document.addEventListener('pointermove', move)
    document.addEventListener('pointerup', up)
    document.addEventListener('pointercancel', up)
  }

  // --- Crop, sample, show ---
  const fmtTime = (m: number) => clock(axis0 + m)

  function commitCrop(which: 'start' | 'end', el: HTMLInputElement) {
    let v = parseClock(el.value, axis0)
    if (v !== null && which === 'end' && v <= app.crop.start) v += 1440
    if (v === null || v < 0 || v > app.axisLen) {
      el.value = fmtTime(app.crop[which])
      return
    }
    if (which === 'start') app.setCrop(true, v, Math.max(app.crop.end, v + 1))
    else app.setCrop(true, app.crop.start, v)
    el.value = fmtTime(app.crop[which])
  }

  function commitCount(el: HTMLInputElement, cur: number | null, set: (n: number | null) => void) {
    const v = parseCount(el.value)
    if (v !== undefined) set(v)
    const now = v === undefined ? cur : v
    el.value = now === null ? '' : now.toLocaleString()
  }

  // Legend for the gutter ramp of the primary sort key.
  const rampLegend = $derived.by(() => {
    const key = app.sort[0]
    const r = app.sortRange
    if (!key || !r) return null
    const stops = Array.from({ length: 9 }, (_, i) => ramp(key.desc ? 1 - i / 8 : i / 8)).join(', ')
    // Values left to right; the gutter flips the ramp for descending keys
    // (so the top of a window always starts at the same end of it).
    const [lo, hi] = [r.min, r.max]
    return {
      name: colName(h, key.col),
      lo: formatValue(h, key.col, lo),
      hi: formatValue(h, key.col, hi),
      gradient: `linear-gradient(to right, ${stops})`,
    }
  })

  const sampled = $derived(app.windows.some((w) => w.count !== null && w.matched !== null && w.count < w.matched))
</script>

<div class="controls">
  <div class="group sortgroup">
    <span class="label muted">Sort</span>
    <div class="chips" bind:this={chipsEl} role="list" aria-label="Sort keys (drag to reorder)">
      {#each shown as { i, k }, pos (JSON.stringify(k.col))}
        <span
          class="chip sort"
          class:dragging={dragging === i}
          class:partial={missingIn(i).length > 0}
          role="listitem"
          title={(missingIn(i).length ? `Not in ${missingIn(i).join(', ')}: those windows skip this key. ` : '') + 'Drag to reorder; click to flip direction; double-click to change'}
          animate:flip={{ duration: 120 }}
          onpointerdown={(e) => onChipDown(e, i)}
        >
          <span class="rank">{pos + 1}</span>
          <button class="ghost text" onclick={(e) => onChipClick(e, i)} ondblclick={() => onChipDblClick(i)}>{describeSort(h, k)}</button>
          <button class="x" aria-label="Remove sort key" onclick={() => removeSort(i)}>×</button>
        </span>
      {/each}
      <button class="add" onclick={() => (picking = picking ? null : { index: null })}>+ sort</button>
      {#if rampLegend}
        <span class="ramp" title="Colours of the sort-key strip beside each window ({rampLegend.name})">
          <span class="muted small">{rampLegend.lo}</span>
          <span class="bar" style:background={rampLegend.gradient}></span>
          <span class="muted small">{rampLegend.hi}</span>
        </span>
      {/if}
    </div>
    {#if picking}
      <div class="anchor">
        {#key picking.index}
          <ColumnPicker
            purpose="sort"
            initial={picking.index !== null ? (app.sort[picking.index]?.col ?? null) : null}
            onclose={() => (picking = null)}
            onpick={pickSort}
          />
        {/key}
      </div>
    {/if}
  </div>

  <div class="group" class:off={!app.crop.on}>
    <label class="label check" title="Crop the time axis of all windows">
      <input type="checkbox" checked={app.crop.on} onchange={(e) => app.setCrop(e.currentTarget.checked)} />
      Crop
    </label>
    <input
      class="time"
      aria-label="Crop start"
      value={fmtTime(app.crop.start)}
      onchange={(e) => commitCrop('start', e.currentTarget)}
      onkeydown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
    />
    <span class="muted">–</span>
    <input
      class="time"
      aria-label="Crop end"
      value={fmtTime(app.crop.end)}
      onchange={(e) => commitCrop('end', e.currentTarget)}
      onkeydown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
    />
  </div>

  <div class="group">
    <label class="label" for="sample-n" title="Randomly keep this many of each window's filtered persons">Sample</label>
    <input
      id="sample-n"
      class="num"
      placeholder="all"
      value={app.sample.n?.toLocaleString() ?? ''}
      title="Persons kept per window after the filter, e.g. 500, 10k (empty: all)"
      onchange={(e) => commitCount(e.currentTarget, app.sample.n, (n) => app.setSample(n))}
      onkeydown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
    />
    {#if app.sample.n}
      <button class="ghost icon" title="Draw a new random sample" aria-label="New random sample" onclick={() => app.reshuffle()}><Icon name="refresh" size={13} /></button>
    {/if}
    {#if app.sample.n && !sampled}
      <span class="muted small" title="Every window has at most this many persons">(all fit)</span>
    {/if}
  </div>

  <div class="group">
    <label class="label" for="show-n" title="Rows per window; scroll to move through the rest">Show</label>
    <input
      id="show-n"
      class="num"
      placeholder="all"
      value={app.show?.toLocaleString() ?? ''}
      title="Rows shown per window, e.g. 1, 50, 1k (empty: all). Scroll to move through them."
      onchange={(e) => commitCount(e.currentTarget, app.show, (n) => app.setShow(n))}
      onkeydown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
    />
    {#if app.show !== null}
      <span class="muted small">of {count(app.maxCount())}</span>
    {/if}
  </div>
</div>

<style>
  .controls {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px 18px;
    padding: 4px 10px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }
  .group {
    position: relative;
    display: flex;
    align-items: center;
    gap: 5px;
  }
  .sortgroup {
    flex: 1 1 320px;
    min-width: 0;
  }
  .group.off .time {
    opacity: 0.55;
  }
  .label {
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    cursor: pointer;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    align-items: center;
    min-height: 22px;
    min-width: 0;
  }
  .chip.sort {
    cursor: grab;
    touch-action: none;
    user-select: none;
  }
  .chip.sort.partial {
    border-style: dashed;
  }
  .chip.sort.dragging {
    cursor: grabbing;
    outline: 1px solid var(--accent);
    opacity: 0.8;
  }
  .chip .text {
    padding: 0;
    background: transparent;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rank {
    font-size: 10px;
    color: var(--muted);
  }
  .time {
    width: 76px;
    padding: 1px 6px;
    font-variant-numeric: tabular-nums;
  }
  .num {
    width: 72px;
    padding: 1px 6px;
    font-variant-numeric: tabular-nums;
  }
  .icon {
    padding: 2px 4px;
  }
  .ramp {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-left: 6px;
  }
  .ramp .bar {
    width: 54px;
    height: 8px;
    border-radius: 2px;
    border: 1px solid var(--line);
  }
  .anchor {
    margin-top: 4px;
  }
</style>
