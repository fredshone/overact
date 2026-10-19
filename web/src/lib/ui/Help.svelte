<script lang="ts">
  import { app } from '../state.svelte'

  const keys: [string, string][] = [
    ['Scroll', 'move through persons (when Show limits the rows)'],
    ['Hover a schedule', "a person's details (zoomed out: the first person in that pixel row)"],
    ['Click a schedule', 'pin that person'],
    ['Drag the grip in a window title', 'move the window (or focus it and press ← / →)'],
    ['Drag on a time axis', 'select a time range: zoom to it, or filter the window on it'],
    ['Wheel on a time axis', 'zoom the time axis of all windows (Shift-wheel or Shift-drag: pan)'],
    ['+ / − / 0', 'zoom in / out / show the whole day'],
    ['← / →', 'pan the zoomed time axis'],
    ['Double-click a time axis', 'show the whole day'],
    ['↑ / ↓, PgUp / PgDn', 'scroll persons'],
    ['Home / End', 'scroll to the first / last persons'],
    ['F', 'switch Fit / Rows scale'],
    ['N', 'new window'],
    ['M', 'cycle mean / majority / sample colour'],
    ['T', 'cycle theme'],
    ['P', 'show / hide profile strips'],
    ['Drag the bar under a strip', 'resize the strips of all windows (double-click: reset)'],
    ['?', 'this help'],
  ]
</script>

<div class="backdrop" role="presentation" onclick={() => (app.showHelp = false)}>
  <div class="popover help" role="dialog" aria-label="Help" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.key === 'Escape' && (app.showHelp = false)}>
    <h2>Keyboard and mouse</h2>
    <table>
      <tbody>
        {#each keys as [k, d] (k)}
          <tr><td><kbd>{k}</kbd></td><td>{d}</td></tr>
        {/each}
      </tbody>
    </table>
    <h2>Reading the view</h2>
    <p>
      Each row is one person's day, coloured by activity. When there are more persons than pixel rows, each
      pixel row averages up to {app.maxSamples} evenly spaced persons (an exact pass follows when you stop
      moving). The strip above each window shows the exact share of each activity by time of day; hover it or the schedules for the mix at that time.
    </p>
    <p>
      The bar under the title holds settings shared by every window: <b>Sort</b> keys (drag a key to change
      the order, click it to flip direction, double-click it to change it; the coloured bar beside it is the
      scale of the strip at the left of each window), <b>Crop</b> to a time range (or zoom on a time axis), <b>Sample</b> to keep a random
      number of each window's filtered persons, and <b>Show</b> to fix how many rows each window displays.
      With few rows each person's schedule is drawn as a tall band; scroll to move through the rest.
    </p>
    <p>
      <b>Fit</b> stretches each window's population to its height, so the same height is the same percentile in
      every window. <b>Rows</b> gives every window the same number of persons per pixel row.
    </p>
    <p>
      <b>+ Dataset</b> loads another dataset with the same time axis to compare with. Each window then has a
      dataset menu, and its ⋯ menu can clone it onto another dataset. Activities, columns and categories are
      matched by name; a sort key one dataset lacks is skipped in its windows (its chip is dashed).
    </p>
    <button class="primary" onclick={() => (app.showHelp = false)}>Close</button>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 100;
    background: rgb(0 0 0 / 0.3);
    display: grid;
    place-items: center;
    padding: 16px;
  }
  .help {
    position: relative;
    width: min(560px, 100%);
    max-height: 90vh;
    overflow: auto;
  }
  h2 {
    font-size: 14px;
    margin: 4px 0 8px;
  }
  td {
    padding: 2px 10px 2px 0;
    vertical-align: top;
  }
  p {
    margin: 8px 0;
  }
</style>
