<script lang="ts">
  import { outside } from './outside'
  import { app } from '../state.svelte'
  import { clock, count, number } from '../format'

  let { onclose }: { onclose: () => void } = $props()
  let picked = $state(app.datasets[0].id)
  const ds = $derived(app.datasets.find((d) => d.id === picked) ?? app.datasets[0])
  const h = $derived(ds.header)
  type Example = { fix: string; person_id: string; start: number; end: number; activity: string }
  const meta = $derived(h.meta as { fixes?: Record<string, number>; fix_examples?: Example[]; warnings?: string[]; packer?: string; loader?: string; created?: string; subset?: { fraction: number; of: number } })
  const FIX_TEXT: Record<string, string> = {
    dropped_missing: 'rows with a missing id, activity, start or end were dropped',
    dropped_empty: 'rows with end ≤ start were dropped',
    dropped_outside: 'rows entirely outside the time axis were dropped',
    clipped: 'rows partly outside the time axis were clipped',
    overlap_trimmed: 'rows overlapping an earlier row had their start moved later',
    overlap_dropped: 'rows fully covered by an earlier row were dropped',
    gap_filled: "gaps were filled with 'unknown'",
    person_filled: "persons with no activities got a whole day of 'unknown'",
    // Older packers merged back-to-back repeats; their files still report it.
    merged: 'back-to-back rows with the same activity were merged',
  }

</script>

<div class="popover info" use:outside={onclose} role="dialog" aria-label="Dataset summary">
  {#if app.datasets.length > 1}
    <div class="tabs" role="tablist">
      {#each app.datasets as d (d.id)}
        <button class="ghost" role="tab" class:on={d.id === ds.id} aria-selected={d.id === ds.id} onclick={() => (picked = d.id)}>{d.name}</button>
      {/each}
    </div>
  {/if}
  <div class="head">
    <h3>{ds.name}</h3>
    {#if app.datasets.length > 1}
      <button class="ghost" title="Close this dataset and its windows" onclick={() => app.removeDataset(ds.id)}>Remove</button>
    {/if}
  </div>
  <table>
    <tbody>
      <tr><td>Persons</td><td>{h.n_persons.toLocaleString()}{meta.subset ? ` (random ${Math.round(meta.subset.fraction * 100)}% of ${count(meta.subset.of)})` : ''}</td></tr>
      <tr><td>Segments</td><td>{h.n_segments.toLocaleString()} ({(h.n_segments / Math.max(1, h.n_persons)).toFixed(1)} per person)</td></tr>
      <tr><td>Time axis</td><td>{clock(h.time_axis.start)}–{clock(h.time_axis.end)}</td></tr>
      <tr><td>Loaded in</td><td>{(ds.loadMs / 1000).toFixed(1)} s</td></tr>
      <tr>
        <td>Engine</td>
        <td title={app.threads > 1 ? 'Multi-threaded (the page is cross-origin isolated)' : 'Single-threaded (the page is not cross-origin isolated)'}>
          {app.threads} thread{app.threads > 1 ? 's' : ''} · {((app.memoryOf[ds.id] ?? 0) / 1e6).toFixed(0)} MB WASM memory · last draw {app.drawMs.toFixed(0)} ms
        </td>
      </tr>
      {#if meta.packer}<tr><td>Packed by</td><td>{meta.packer}{meta.created ? `, ${meta.created.slice(0, 10)}` : ''}</td></tr>{/if}
      {#if meta.loader}<tr><td>Read by</td><td>{meta.loader} (straight from the tables)</td></tr>{/if}
    </tbody>
  </table>
  <h4>Activities ({h.activities.length})</h4>
  <p>{h.activities.map((a) => a.label).join(', ')}</p>
  <h4>Columns ({h.columns.length})</h4>
  <table class="cols">
    <tbody>
      {#each h.columns as c (c.name)}
        <tr>
          <td>{c.name}</td>
          <td class="muted">
            {#if c.kind === 'categorical'}{c.categories?.length} categories{:else if c.kind === 'boolean'}boolean{:else if c.min !== null}{number(c.min)} – {number(c.max ?? NaN)}{:else}empty{/if}
          </td>
          <td class="muted">{c.nullCount ? `${count(c.nullCount)} missing` : ''}</td>
        </tr>
      {/each}
    </tbody>
  </table>
  {#if meta.fixes && Object.keys(meta.fixes).length}
    <h4>Fixes made while {meta.loader ? 'loading' : 'packing'}</h4>
    <ul>
      {#each Object.entries(meta.fixes) as [k, v] (k)}
        <li>
          <span title={FIX_TEXT[k] ?? ''}>{v.toLocaleString()} {FIX_TEXT[k] ?? k.replace(/_/g, ' ')}</span>
          {#if meta.fix_examples?.some((x) => x.fix === k)}
            <details>
              <summary class="muted">examples</summary>
              <table class="ex">
                <tbody>
                  {#each meta.fix_examples.filter((x) => x.fix === k) as x, i (i)}
                    <tr><td>{x.person_id}</td><td>{clock(x.start)}–{clock(x.end)}</td><td>{x.activity}</td></tr>
                  {/each}
                </tbody>
              </table>
            </details>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
  {#if meta.warnings?.length}
    <h4>Warnings</h4>
    <ul>{#each meta.warnings as w, i (i)}<li>{w}</li>{/each}</ul>
  {/if}
</div>

<style>
  .ex {
    font-size: 11px;
    font-family: var(--mono);
    margin: 2px 0 6px;
  }
  .ex td {
    padding: 0 8px 0 0;
  }
  details summary {
    cursor: pointer;
    font-size: 11px;
  }
  .info {
    left: 10px;
    top: 40px;
    width: min(460px, calc(100vw - 20px));
    max-height: 75vh;
    overflow: auto;
  }
  .tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 2px;
    margin-bottom: 8px;
    border-bottom: 1px solid var(--line);
  }
  .tabs button.on {
    font-weight: 600;
    box-shadow: inset 0 -2px var(--accent);
  }
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }
  h3 {
    margin: 0 0 6px;
    font-size: 14px;
  }
  h4 {
    margin: 10px 0 4px;
    font-size: 12px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  td {
    padding: 1px 10px 1px 0;
    vertical-align: top;
  }
  .cols td:first-child {
    font-family: var(--mono);
    font-size: 12px;
  }
  p,
  ul {
    margin: 0;
  }
  ul {
    padding-left: 18px;
  }
</style>
