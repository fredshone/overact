<script lang="ts">
  // The schedule section of the filter picker. Every test is built from the
  // same steps: what to test, then which activities (each ticked box is one
  // condition; together they must all hold).
  import { app } from '../state.svelte'
  import type { Condition, Header } from '../engine/types'
  import { clock, describeCondition, parseClock } from '../format'

  let { header, onconditions }: { header: Header; onconditions: (cs: Condition[]) => void } = $props()

  type Tab = 'contains' | 'doing'
  const TABS: { t: Tab; label: string; title: string }[] = [
    { t: 'contains', label: 'Contains', title: 'Does the activity at least once in the day' },
    { t: 'doing', label: 'Doing', title: 'Is doing the activity at a time, or during a time window' },
  ]
  let tab = $state<Tab>('contains')

  const h = $derived(header)
  const axis0 = $derived(h.time_axis.start)

  // Activities ticked (`not`: something other than the activity).
  let ticked = $state<{ activity: number; not: boolean }[]>([])
  const isTicked = (activity: number, not: boolean) => ticked.some((t) => t.activity === activity && t.not === not)
  function toggle(activity: number, not: boolean) {
    ticked = isTicked(activity, not)
      ? ticked.filter((t) => t.activity !== activity || t.not !== not)
      : [...ticked, { activity, not }]
  }
  const sortedTicks = $derived([...ticked].sort((a, b) => a.activity - b.activity || Number(a.not) - Number(b.not)))

  // Doing: at a minute, at any time during a window, or throughout it.
  let when = $state<'at' | 'any' | 'all'>('any')
  let startText = $state('09:00')
  let endText = $state('17:00')
  const start = $derived(parseClock(startText, axis0))
  const end = $derived.by(() => {
    let e = parseClock(endText, axis0)
    if (e !== null && start !== null && e <= start) e += 1440
    return e
  })
  const timesOk = $derived(
    start !== null && start >= 0 && start < app.axisLen && (when === 'at' || (end !== null && end <= app.axisLen)),
  )


  const conditions = $derived.by<Condition[]>(() => {
    if (tab === 'contains') {
      return sortedTicks.map(({ activity, not }) => ({
        col: { feature: 'measure', measure: 'count', activity, not, nth: 0 },
        negate: false,
        type: 'range',
        min: 1,
        max: null,
      }))
    }
    if (!timesOk) return []
    // Each minute has exactly one activity, so "doing something other than
    // X" is "not doing X": at any time during = not X throughout, and
    // throughout = not X at any time.
    return sortedTicks.map(({ activity, not }) => ({
      col:
        when === 'at'
          ? { feature: 'doing_at', activity, minute: start! }
          : { feature: 'doing_during', activity, start: start!, end: end!, all: (when === 'all') !== not },
      negate: not,
      type: 'in',
      values: [1],
    }))
  })

  const hint = $derived(
    tab === 'doing' && !timesOk ? 'Enter valid times' : 'Tick one or more activities',
  )
</script>

<div class="builder" role="group" aria-label="Schedule filter">
  <div class="group">Schedule</div>
  <div class="seg" role="radiogroup" aria-label="Test">
    {#each TABS as { t, label, title } (t)}
      <button role="radio" aria-checked={tab === t} class:on={tab === t} {title} onclick={() => (tab = t)}>{label}</button>
    {/each}
  </div>

  {#if tab === 'doing'}
    <div class="sentence">
      <select bind:value={when} aria-label="When">
        <option value="at">at</option>
        <option value="any">at any time during</option>
        <option value="all">throughout</option>
      </select>
      <input class="time" aria-label="Start time" bind:value={startText} />
      {#if when !== 'at'}
        <span class="muted">–</span>
        <input class="time" aria-label="End time" bind:value={endText} />
      {/if}
    </div>
  {/if}

  <div class="acts" role="group" aria-label="Activities">
      <span class="muted head">activity</span>
      <span class="muted head" title="Something other than this activity">not</span>
      {#each h.activities as a (a.code)}
        <label class="act"><input type="checkbox" checked={isTicked(a.code, false)} onchange={() => toggle(a.code, false)} /> {a.label}</label>
        <input type="checkbox" aria-label="not {a.label}" checked={isTicked(a.code, true)} onchange={() => toggle(a.code, true)} />
      {/each}
    </div>

  <div class="use">
    <span class="preview">
      {conditions.length ? conditions.map((c) => describeCondition(h, c)).join(' and ') : hint}
    </span>
    <button class="primary" disabled={!conditions.length} onclick={() => onconditions(conditions)}>Filter</button>
  </div>
  {#if tab === 'doing' && timesOk && axis0 % 1440 !== 0}
    <div class="muted small">The day starts at {clock(axis0)}: earlier times mean the next day.</div>
  {/if}
</div>

<style>
  .builder {
    display: grid;
    gap: 6px;
    border-top: 1px solid var(--line);
    padding-top: 2px;
  }
  .group {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted);
    padding: 6px 0 2px;
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
  .time {
    width: 64px;
    padding: 1px 6px;
    font-variant-numeric: tabular-nums;
  }
  .acts {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: center;
    column-gap: 10px;
    max-height: 220px;
    overflow: auto;
    padding-right: 4px;
  }
  .acts .head {
    font-size: 11px;
  }
  .act {
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
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
  .small {
    font-size: 11px;
  }
</style>
