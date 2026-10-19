<script lang="ts">
  import type { Snippet } from 'svelte'
  import type { Inspection } from '../engine/types'
  import { count } from '../format'
  import { formatBytes } from '../roles'

  let {
    title,
    note,
    file,
    info,
    error = '',
    busy = false,
    highlight = [],
    onfile,
    children,
  }: {
    title: string
    note: string
    file: File | null
    info: Inspection | null
    error?: string
    busy?: boolean
    /** Column names to highlight in the preview (mapped roles). */
    highlight?: string[]
    onfile: (f: File | null) => void
    children?: Snippet
  } = $props()

  let input = $state<HTMLInputElement>()
  let dragging = $state(false)
  const PREVIEW_ROWS = 3
</script>

<section
  class="cell"
  class:dragging
  class:filled={!!file}
  aria-label={title}
  ondragover={(e) => {
    e.preventDefault()
    e.stopPropagation()
    dragging = true
  }}
  ondragleave={() => (dragging = false)}
  ondrop={(e) => {
    e.preventDefault()
    e.stopPropagation()
    dragging = false
    const f = e.dataTransfer?.files[0]
    if (f) onfile(f)
  }}
>
  <header>
    <h2>{title}</h2>
    <span class="muted">{note}</span>
  </header>

  {#if !file}
    <button class="drop" onclick={() => input?.click()}>
      <strong>Drop a CSV or Parquet file</strong>
      <span class="muted">or click to choose</span>
    </button>
  {:else}
    <div class="file">
      <div class="name" title={file.name}>
        <strong>{file.name}</strong>
        <span class="muted">
          {formatBytes(file.size)}{#if info}{' · '}{info.format.toUpperCase()} · {info.columns.length} columns{#if info.n_rows !== null}{' · '}{count(info.n_rows)} rows{/if}{/if}
        </span>
      </div>
      <button class="ghost" title="Choose another file" onclick={() => input?.click()}>Change</button>
      <button class="ghost" title="Remove" aria-label="Remove {title.toLowerCase()} file" onclick={() => onfile(null)}>×</button>
    </div>
    {#if busy}
      <p class="muted">Reading columns…</p>
    {:else if error}
      <p class="error">{error}</p>
    {:else if info}
      <div class="preview" role="region" aria-label="First rows" tabindex="-1">
        <table>
          <thead>
            <tr>{#each info.columns as c (c.name)}<th class:hl={highlight.includes(c.name)} title={c.type}>{c.name}</th>{/each}</tr>
          </thead>
          <tbody>
            {#each info.preview.slice(0, PREVIEW_ROWS) as row, i (i)}
              <tr>{#each row as v, j (j)}<td class:hl={highlight.includes(info.columns[j].name)} class:null={v === null}>{v ?? '–'}</td>{/each}</tr>
            {/each}
          </tbody>
        </table>
      </div>
      {@render children?.()}
    {/if}
  {/if}
  <input
    bind:this={input}
    type="file"
    accept=".csv,.tsv,.tab,.txt,.parquet,.pq"
    hidden
    onchange={(e) => {
      const f = e.currentTarget.files?.[0]
      e.currentTarget.value = ''
      if (f) onfile(f)
    }}
  />
</section>

<style>
  .cell {
    display: grid;
    gap: 10px;
    align-content: start;
    min-width: 0;
    padding: 14px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--bg);
  }
  .cell.dragging {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, var(--bg));
  }
  header {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  h2 {
    margin: 0;
    font-size: 15px;
  }
  .drop {
    display: grid;
    gap: 4px;
    padding: 28px 12px;
    border: 2px dashed var(--line);
    background: transparent;
    text-align: center;
  }
  .drop:hover {
    border-color: var(--accent);
    background: transparent;
  }
  .file {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .name {
    flex: 1;
    min-width: 0;
    display: grid;
  }
  .name > * {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .preview {
    overflow-x: auto;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
  }
  table {
    border-collapse: collapse;
    font-size: 11px;
    font-family: var(--mono);
  }
  th,
  td {
    padding: 3px 8px;
    text-align: left;
    white-space: nowrap;
    max-width: 160px;
    overflow: hidden;
    text-overflow: ellipsis;
    border-bottom: 1px solid var(--line);
  }
  th {
    font-weight: 600;
    color: var(--muted);
  }
  tbody tr:last-child td {
    border-bottom: 0;
  }
  .hl {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    color: var(--fg);
  }
  td.null {
    color: var(--muted);
  }
  p {
    margin: 0;
  }
  .error {
    color: var(--danger);
  }
</style>
