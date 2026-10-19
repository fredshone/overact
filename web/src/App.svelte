<script lang="ts">
  import { untrack } from 'svelte'
  import { app, MAX_WINDOWS } from './lib/state.svelte'
  import Icon from './lib/ui/Icon.svelte'
  import Notices from './lib/ui/Notices.svelte'
  import Loader from './lib/ui/Loader.svelte'
  import Window from './lib/ui/Window.svelte'
  import Legend from './lib/ui/Legend.svelte'
  import Help from './lib/ui/Help.svelte'
  import MainMenu from './lib/ui/MainMenu.svelte'
  import DatasetInfo from './lib/ui/DatasetInfo.svelte'
  import type { RenderMode } from './lib/engine/types'
  import Toolbar from './lib/ui/Toolbar.svelte'
  import { encodeHash } from './lib/viewconfig'
  import { count } from './lib/format'

  // Theme: data-theme on <html> is always the resolved theme ("system"
  // follows prefers-color-scheme), so the CSS needs one dark block. Canvases
  // can't use CSS variables: re-read the colours and redraw.
  $effect(() => {
    document.documentElement.dataset.theme = app.resolvedTheme
    if (!app.datasets.length) return
    untrack(() => {
      app.readThemeColors()
      app.pushPalette()
    })
  })

  function onKey(e: KeyboardEvent) {
    const tag = (e.target as HTMLElement)?.tagName
    if (tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA' || e.ctrlKey || e.metaKey || e.altKey) return
    if (e.key === '?') {
      app.showHelp = !app.showHelp
      return
    }
    if (e.key === 'Escape') app.showHelp = false
    if (!app.datasets.length || app.adding) return
    const h = app.windowHeight
    const handled = (() => {
      switch (e.key) {
        case 'ArrowUp':
          app.scrollRows(-h * 0.1, h)
          return true
        case 'ArrowDown':
          app.scrollRows(h * 0.1, h)
          return true
        case 'PageUp':
          app.scrollRows(-h * 0.9, h)
          return true
        case 'PageDown':
          app.scrollRows(h * 0.9, h)
          return true
        case 'Home':
          app.scrollTo(0)
          return true
        case 'End':
          app.scrollTo(1)
          return true
        case 'f':
        case 'F':
          app.setVerticalMode(app.vertical.mode === 'fit' ? 'rows' : 'fit')
          return true
        case 'n':
        case 'N':
          app.addWindow()
          return true
        case 'm':
        case 'M':
          app.renderMode = ((app.renderMode + 1) % 3) as RenderMode
          return true
        case 't':
        case 'T':
          app.cycleTheme()
          return true
        case '+':
        case '=':
          app.zoomTime(0.5)
          return true
        case '-':
          app.zoomTime(2)
          return true
        case '0':
          app.resetTime()
          return true
        case 'ArrowLeft':
        case 'ArrowRight': {
          const span = app.time.t1 - app.time.t0
          app.panTime((e.key === 'ArrowLeft' ? -0.2 : 0.2) * span)
          return true
        }
        case 'p':
        case 'P':
          app.showProfiles = !app.showProfiles
          return true
      }
      return false
    })()
    if (handled) e.preventDefault()
  }

  let menuOpen = $state(false)
  let infoOpen = $state(false)

  // Keep the URL hash in sync with the view (small configs only, never data).
  let hashTimer: ReturnType<typeof setTimeout> | undefined
  $effect(() => {
    if (!app.datasets.length || !app.header) return
    const cfg = app.toConfig()
    clearTimeout(hashTimer)
    hashTimer = setTimeout(() => {
      const hash = encodeHash(cfg)
      history.replaceState(null, '', hash ?? location.pathname + location.search)
    }, 400)
  })

  const only = $derived(app.datasets.length === 1 ? app.datasets[0] : null)
  const subset = $derived(only?.header.meta?.subset as { fraction: number; of: number } | undefined)
</script>

<svelte:window onkeydown={onKey} />

{#if !app.datasets.length}
  <Loader />
{:else}
  <div class="shell">
    <header class="bar">
      <strong class="brand">overact</strong>
      <button class="ghost ds muted" title="Dataset summary" onclick={() => (infoOpen = !infoOpen)}>
        {#if only}
          {only.name} · {count(only.header.n_persons)} persons{subset ? ` (${Math.round(subset.fraction * 100)}% of ${count(subset.of)})` : ''} ▾
        {:else}
          {app.datasets.map((d) => d.name).join(' · ')} ▾
        {/if}
      </button>
      <button class="ghost with-icon" onclick={() => app.startAdding()} title="Load another dataset to compare with"><Icon name="plus" size={13} /> Dataset</button>
      <span class="spacer"></span>
      <div class="seg" role="group" aria-label="Vertical scale">
        <button class:on={app.vertical.mode === 'fit'} onclick={() => app.setVerticalMode('fit')} title="Each window's population fills its height (percentiles line up)">Fit</button>
        <button class:on={app.vertical.mode === 'rows'} onclick={() => app.setVerticalMode('rows')} title="Same persons per pixel row in every window">Rows</button>
      </div>
      <div class="seg" role="group" aria-label="Colour mode">
        <button class:on={app.renderMode === 0} onclick={() => (app.renderMode = 0)} title="Average colour of the persons in each pixel">Mean</button>
        <button class:on={app.renderMode === 1} onclick={() => (app.renderMode = 1)} title="Most common activity in each pixel">Majority</button>
        <button class:on={app.renderMode === 2} onclick={() => (app.renderMode = 2)} title="Activity of the first person in each pixel row">Sample</button>
      </div>
      <button class="with-icon" onclick={() => app.addWindow()} disabled={app.windows.length >= MAX_WINDOWS} title="New window (N)"><Icon name="plus" size={13} /> Window</button>
      <button class="ghost icon" onclick={() => app.cycleTheme()} title="Theme: {app.theme} (T)" aria-label="Theme: {app.theme}"><Icon name="theme" /></button>
      <button class="ghost icon" onclick={() => (app.showHelp = true)} title="Help (?)" aria-label="Help"><Icon name="help" /></button>
      <button class="ghost icon" onclick={() => (menuOpen = !menuOpen)} title="View, export and display options" aria-label="Menu" aria-haspopup="menu" aria-expanded={menuOpen}><Icon name="menu" /></button>
      <button class="ghost" onclick={() => app.close()} title={only ? 'Close the dataset' : 'Close all datasets'}>Close</button>
    </header>
    {#if menuOpen}<MainMenu onclose={() => (menuOpen = false)} />{/if}
    {#if infoOpen}<DatasetInfo onclose={() => (infoOpen = false)} />{/if}
    <Toolbar />
    <div class="legendbar"><Legend /></div>
    <main class="windows" style:grid-template-columns="repeat({app.windows.length}, minmax(0, 1fr))">
      {#each app.windows as w (w.id)}
        <Window win={w} />
      {/each}
    </main>
  </div>
  {#if app.adding}
    <div class="adding" role="dialog" aria-label="Add a dataset"><Loader /></div>
  {/if}
{/if}

<Notices />

{#if app.showHelp}
  <Help />
{/if}

<style>
  .shell {
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
    flex-wrap: wrap;
  }
  .legendbar {
    padding: 4px 10px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }
  .brand {
    letter-spacing: -0.01em;
  }
  .ds {
    max-width: 260px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
  }
  .adding {
    position: fixed;
    inset: 0;
    z-index: 50;
    overflow: auto;
    background: var(--bg);
  }
  .windows {
    flex: 1;
    min-height: 0;
    display: grid;
    gap: 8px;
    padding: 8px;
  }
  @media (max-width: 700px) {
    .windows {
      grid-template-columns: minmax(0, 1fr) !important;
      grid-auto-rows: minmax(300px, 1fr);
      overflow: auto;
    }
  }
</style>
