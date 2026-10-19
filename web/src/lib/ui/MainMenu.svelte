<script lang="ts">
  import { outside } from './outside'
  import { app, SAMPLE_CHOICES, type Dataset } from '../state.svelte'
  import { exportPNG, exportSVG, saveConfig, saveOveract } from '../export'
  import { encodeHash, isViewConfig } from '../viewconfig'

  let { onclose }: { onclose: () => void } = $props()
  let fileInput: HTMLInputElement | undefined = $state()
  let copied = $state('')
  let saving = $state(false)

  async function saveData(ds: Dataset) {
    saving = true
    try {
      await saveOveract(ds)
      onclose()
    } catch (e) {
      app.notify(`Could not save the dataset: ${e instanceof Error ? e.message : e}`)
    } finally {
      saving = false
    }
  }


  async function loadConfig(f: File | undefined) {
    if (!f) return
    try {
      const cfg = JSON.parse(await f.text())
      if (!isViewConfig(cfg)) throw new Error('not an overact view file')
      app.applyConfig(cfg)
      onclose()
    } catch (e) {
      app.notify(`Could not load the view: ${e instanceof Error ? e.message : e}`)
    }
  }

  async function copyLink() {
    const hash = encodeHash(app.toConfig())
    if (!hash) {
      copied = 'Too large for a link: use Save view instead'
      return
    }
    const url = `${location.origin}${location.pathname}${hash}`
    try {
      await navigator.clipboard.writeText(url)
      copied = 'Link copied. It holds the view, not the data.'
    } catch {
      copied = url
    }
  }
</script>

<div class="popover menu" use:outside={onclose} role="menu">
  <div class="group">View</div>
  <button class="ghost" role="menuitem" onclick={() => { saveConfig(); onclose() }}>Save view to file…</button>
  <button class="ghost" role="menuitem" onclick={() => fileInput?.click()}>Load view from file…</button>
  <input bind:this={fileInput} type="file" accept=".json,application/json" hidden onchange={(e) => loadConfig(e.currentTarget.files?.[0])} />
  <button class="ghost" role="menuitem" onclick={copyLink}>Copy link to this view</button>
  {#if copied}<div class="note muted">{copied}</div>{/if}
  <div class="group">Export</div>
  <button class="ghost" role="menuitem" onclick={() => { onclose(); exportPNG() }}>PNG of all windows</button>
  <button class="ghost" role="menuitem" onclick={() => { onclose(); exportSVG() }}>SVG of legend and profiles</button>
  {#each app.datasets as d (d.id)}
    <button class="ghost" role="menuitem" onclick={() => saveData(d)} disabled={saving} title="Save the loaded dataset as one file that opens without parsing">
      {saving ? 'Saving…' : app.datasets.length > 1 ? `“${d.name}” as .overact file` : 'Dataset as .overact file'}
    </button>
  {/each}
  <div class="group">Display</div>
  <label class="row">Samples per pixel row
    <select bind:value={app.maxSamples}>
      {#each SAMPLE_CHOICES as k (k)}<option value={k}>{k}</option>{/each}
    </select>
  </label>
  <label class="row"><input type="checkbox" bind:checked={app.exactPass} /> Exact pass when idle</label>
  <label class="row"><input type="checkbox" bind:checked={app.showProfiles} /> Profile strips</label>
</div>

<style>
  .menu {
    right: 8px;
    top: 40px;
    display: grid;
    min-width: 250px;
    max-width: 320px;
  }
  .menu > button {
    text-align: left;
  }
  .group {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted);
    padding: 8px 8px 2px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 8px;
    justify-content: space-between;
  }
  .row:has(input[type='checkbox']) {
    justify-content: flex-start;
  }
  .note {
    font-size: 11px;
    padding: 2px 8px;
    word-break: break-all;
  }
</style>
