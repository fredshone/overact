<script lang="ts">
  import { app } from '../state.svelte'
  import { defaultColors } from '../palette'
  import { pct } from '../format'

  // Share of all persons' time in each activity (for the tooltips).
  const total = $derived(app.activityMinutes.reduce((a, b) => a + b, 0))
  const share = (code: number) => (total ? pct((app.activityMinutes[code] ?? 0) / total) : '')

  function resetColors() {
    const d = defaultColors(app.activities.map((a) => a.label))
    app.activities.forEach((a, i) => (a.color = d[i]))
    app.pushPalette()
    app.saveColors()
  }

  function toggle(code: number) {
    const a = app.activities.find((x) => x.code === code)!
    a.visible = !a.visible
    app.pushPalette()
  }

  function solo(code: number) {
    const only = app.activities.every((a) => a.visible === (a.code === code))
    app.activities.forEach((a) => (a.visible = only || a.code === code))
    app.pushPalette()
  }
</script>

<div class="legend" role="list" aria-label="Activities">
  {#each app.activities as a (a.code)}
    <div class="item" class:off={!a.visible} role="listitem">
      <label class="swatch" style:background={a.color} title="Change the colour of {a.label}">
        <!-- Live preview while picking (once per frame); the choice is remembered on change. -->
        <input
          type="color"
          bind:value={a.color}
          oninput={() => app.schedulePalette()}
          onchange={() => app.saveColors()}
          aria-label="Colour for {a.label}"
        />
      </label>
      <button
        class="ghost name"
        title="{a.label}{share(a.code) ? `: ${share(a.code)} of all time` : ''}. Click: show/hide. Double-click: show only this"
        onclick={() => toggle(a.code)}
        ondblclick={() => solo(a.code)}
      >{a.label}</button>
    </div>
  {/each}
  <button class="ghost reset muted" onclick={resetColors} title="Reset colours to the defaults">reset</button>
</div>

<style>
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 8px;
    align-items: center;
  }
  .item {
    display: inline-flex;
    align-items: center;
  }
  .item.off .name {
    text-decoration: line-through;
    color: var(--muted);
  }
  .item.off .swatch {
    opacity: 0.3;
  }
  .swatch {
    position: relative;
    width: 18px;
    height: 18px;
    border-radius: 4px;
    border: 1px solid rgb(0 0 0 / 0.2);
    cursor: pointer;
    overflow: hidden;
  }
  .swatch:hover {
    box-shadow: 0 0 0 2px var(--chip-hover);
  }
  .swatch input {
    position: absolute;
    inset: 0;
    opacity: 0;
    cursor: pointer;
  }
  .name {
    padding: 1px 4px;
  }
  .reset {
    font-size: 11px;
  }
</style>
