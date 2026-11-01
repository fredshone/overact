<script lang="ts">
  import { app, type Win } from '../../state.svelte'
  import { count } from '../../format'
  import SequenceChart from './SequenceChart.svelte'
  import StartDuration from './StartDuration.svelte'
  import Participation from './Participation.svelte'
  import Transitions from './Transitions.svelte'
  import ProfileStrip from '../ProfileStrip.svelte'

  let { win }: { win: Win } = $props()
  let el: HTMLDivElement
  let width = $state(0)
  const PAD = 12
  const inner = $derived(Math.max(40, width - 2 * PAD))
  const s = $derived(win.summary)

  // Scrolling is linked across windows; cards depend only on the width, so
  // they stay lined up.
  function onScroll() {
    app.summaryScroll = el.scrollTop
  }
  $effect(() => {
    const y = app.summaryScroll
    if (el && Math.abs(el.scrollTop - y) > 1) el.scrollTop = y
  })
</script>

<div class="panel" bind:this={el} bind:clientWidth={width} onscroll={onScroll} style:padding="{PAD}px">
  {#if app.showProfiles}
    <section class="profile">
      <h3>Activity shares by time of day</h3>
      <ProfileStrip {win} width={inner} indent={0} />
    </section>
  {/if}
  {#if !s}
    <div class="empty muted">{win.count === 0 ? 'No persons match this filter.' : 'Computing summary…'}</div>
  {:else}
    {#if win.summaryPending}
      <div class="pending muted" title="These charts are for the previous rows; new ones follow">updating…</div>
    {/if}
    <div class="cards" class:stale={win.summaryPending}>
      <section>
        <h3>Sequences <span class="muted">{count(s.n_distinct)} distinct, ignoring durations</span></h3>
        <SequenceChart summary={s} width={inner} />
      </section>
      <section>
        <h3>When and how long</h3>
        <StartDuration summary={s} width={inner} />
      </section>
      <section>
        <h3>Participation and time use</h3>
        <Participation summary={s} width={inner} />
      </section>
      <section>
        <h3>Transitions</h3>
        <Transitions summary={s} width={inner} />
      </section>
    </div>
  {/if}
</div>

<style>
  .panel {
    position: absolute;
    inset: 0;
    overflow-y: auto;
    overflow-x: hidden;
    background: var(--panel);
    z-index: 4;
  }
  .empty {
    display: grid;
    place-items: center;
    min-height: 120px;
  }
  .profile {
    margin-bottom: 18px;
  }
  .cards {
    transition: opacity 0.2s;
  }
  .stale {
    opacity: 0.45;
  }
  section + section {
    margin-top: 18px;
    padding-top: 12px;
    border-top: 1px solid var(--line);
  }
  h3 {
    margin: 0 0 8px;
    font-size: 13px;
    font-weight: 600;
  }
  h3 .muted {
    font-weight: normal;
    font-size: 12px;
  }
  .pending {
    position: sticky;
    top: 0;
    float: right;
    z-index: 2;
    font-size: 11px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 0 6px;
  }
</style>
