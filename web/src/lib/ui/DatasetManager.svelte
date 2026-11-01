<script lang="ts">
  import { app, MAX_WINDOWS, type Dataset } from '../state.svelte'
  import { clock, count } from '../format'
  import Icon from './Icon.svelte'

  type Meta = { subset?: { fraction: number; of: number }; packer?: string; loader?: string; created?: string; warnings?: string[] }
  const metaOf = (d: Dataset) => d.header.meta as Meta

  function commitName(d: Dataset, el: HTMLInputElement) {
    app.renameDataset(d.id, el.value)
    // Show what was kept (an empty or duplicate name is not used as typed).
    el.value = app.datasets.find((x) => x.id === d.id)?.name ?? el.value
  }

  function remove(d: Dataset) {
    if (app.datasets.length === 1 && !confirm(`Remove "${d.name}"? It is the only dataset, so all windows close.`)) return
    app.removeDataset(d.id)
  }

  const totalMemory = $derived(app.memory)
</script>

<div class="page">
  <div class="card">
    <div class="head">
      <h1>Datasets</h1>
      <span class="muted">{app.datasets.length} loaded · {(totalMemory / 1e6).toFixed(0)} MB WASM memory</span>
      <span class="spacer"></span>
      <button class="with-icon" onclick={() => app.startAdding()} title="Load another dataset to compare with"><Icon name="plus" size={13} /> Dataset</button>
      <button class="primary" onclick={() => (app.managing = false)}>Done</button>
    </div>
    <p class="muted">
      Each dataset has its own engine and memory. Rename a dataset to change how windows and saved views refer to it;
      removing one closes its windows and frees its memory.
    </p>

    <ul class="list">
      {#each app.datasets as d (d.id)}
        {@const h = d.header}
        {@const meta = metaOf(d)}
        {@const wins = app.windows.filter((w) => w.dataset === d.id)}
        <li class="ds">
          <div class="row">
            <input
              class="name"
              value={d.name}
              aria-label="Dataset name"
              title="Rename this dataset"
              onchange={(e) => commitName(d, e.currentTarget)}
              onkeydown={(e) => {
                if (e.key === 'Enter') e.currentTarget.blur()
                else if (e.key === 'Escape') {
                  e.currentTarget.value = d.name
                  e.currentTarget.blur()
                }
              }}
            />
            <span class="spacer"></span>
            <button class="ghost" disabled={app.windows.length >= MAX_WINDOWS} title="Add a window showing this dataset" onclick={() => app.addWindow(undefined, d.id)}>New window</button>
            <button class="ghost danger" title="Close this dataset and its windows" onclick={() => remove(d)}>Remove</button>
          </div>
          <dl>
            <div><dt>Persons</dt><dd>{h.n_persons.toLocaleString()}{meta.subset ? ` (random ${Math.round(meta.subset.fraction * 100)}% of ${count(meta.subset.of)})` : ''}</dd></div>
            <div><dt>Segments</dt><dd>{count(h.n_segments)} ({(h.n_segments / Math.max(1, h.n_persons)).toFixed(1)} per person)</dd></div>
            <div><dt>Time axis</dt><dd>{clock(h.time_axis.start)}–{clock(h.time_axis.end)}</dd></div>
            <div><dt>Activities</dt><dd title={h.activities.map((a) => a.label).join(', ')}>{h.activities.length}</dd></div>
            <div><dt>Attributes</dt><dd title={h.columns.map((c) => c.name).join(', ')}>{h.columns.length}</dd></div>
            <div><dt>Memory</dt><dd>{((app.memoryOf[d.id] ?? 0) / 1e6).toFixed(0)} MB</dd></div>
            <div><dt>Loaded in</dt><dd>{(d.loadMs / 1000).toFixed(1)} s{meta.loader ? ' from tables' : ''}</dd></div>
            <div>
              <dt>Windows</dt>
              <dd title={wins.map((w) => w.title).join(', ')}>{wins.length ? wins.map((w) => w.title).join(', ') : 'none'}</dd>
            </div>
          </dl>
          {#if meta.warnings?.length}
            <p class="muted small">{meta.warnings.length} warning{meta.warnings.length > 1 ? 's' : ''} while loading (see the dataset summary).</p>
          {/if}
        </li>
      {/each}
    </ul>
  </div>
</div>

<style>
  .page {
    min-height: 100%;
    display: grid;
    justify-items: center;
    align-content: start;
    padding: 32px 16px;
  }
  .card {
    width: min(760px, 100%);
    display: grid;
    gap: 12px;
  }
  .head {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 10px;
  }
  h1 {
    margin: 0;
    font-size: 20px;
  }
  .spacer {
    flex: 1;
  }
  p {
    margin: 0;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 10px;
  }
  .ds {
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 10px 12px;
    display: grid;
    gap: 8px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .name {
    font-weight: 600;
    font-size: 15px;
    flex: 1 1 200px;
    max-width: 360px;
    min-width: 0;
  }
  .danger {
    color: var(--danger);
  }
  dl {
    margin: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
    gap: 6px 16px;
  }
  dl div {
    min-width: 0;
  }
  dt {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  dd {
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .small {
    font-size: 12px;
  }
</style>
