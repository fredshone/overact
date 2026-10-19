<script lang="ts" module>
  // Small line icons (16 px grid, drawn with the current text colour), so
  // they look the same whatever fonts the system has.
  const PATHS = {
    theme: 'M8 2.5a5.5 5.5 0 1 0 0 11zM8 2.5a5.5 5.5 0 1 1 0 11',
    menu: 'M2.5 4.5h11M2.5 8h11M2.5 11.5h11',
    help: 'M8 14a6 6 0 1 0 0-12 6 6 0 0 0 0 12zM6.2 6.3a1.9 1.9 0 1 1 2.6 1.8c-.5.2-.8.6-.8 1.1v.4M8 11.4v.1',
    clone: 'M5.5 5.5h7v7h-7zM10.5 5.5v-2h-7v7h2',
    more: 'M3.5 8h.01M8 8h.01M12.5 8h.01',
    grip: 'M6 3.5h.01M10 3.5h.01M6 8h.01M10 8h.01M6 12.5h.01M10 12.5h.01',
    refresh: 'M13 8a5 5 0 1 1-1.5-3.55M13 2.5v3h-3',
    close: 'M4 4l8 8M12 4l-8 8',
    plus: 'M8 3v10M3 8h10',
    zoomOut: 'M7 12a5 5 0 1 0 0-10 5 5 0 0 0 0 10zM14 14l-3.5-3.5M5 7h4',
  } as const
  export type IconName = keyof typeof PATHS
</script>

<script lang="ts">
  let { name, size = 15 }: { name: IconName; size?: number } = $props()
  // Dots are drawn as zero-length round-capped lines, so they need a thicker stroke.
  const dots = $derived(name === 'more' || name === 'grip')
</script>

<svg
  class="icon"
  width={size}
  height={size}
  viewBox="0 0 16 16"
  fill="none"
  stroke="currentColor"
  stroke-width={dots ? 2.4 : 1.5}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#if name === 'theme'}
    <circle cx="8" cy="8" r="5.5" />
    <path d="M8 2.5a5.5 5.5 0 0 1 0 11z" fill="currentColor" />
  {:else}
    <path d={PATHS[name]} />
  {/if}
</svg>

<style>
  .icon {
    display: block;
    flex: none;
  }
</style>
