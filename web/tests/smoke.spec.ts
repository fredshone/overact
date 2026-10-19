import { expect, test, type Page } from '@playwright/test'
import path from 'node:path'

const SMALL = path.resolve(import.meta.dirname, '../../crates/overact-core/tests/data/synth_small.overact')
const FIX = path.resolve(import.meta.dirname, '../../py/tests/fixtures')

async function open(page: Page, fraction?: string) {
  await page.goto('/')
  if (fraction) await page.getByRole('combobox', { name: 'Load', exact: true }).selectOption(fraction)
  await page.locator('input[accept=".overact"]').setInputFiles(SMALL)
  await expect(page.locator('.window .count b').first()).toBeVisible()
}

/** Loads schedules + attributes tables through the two load cells. */
async function openTables(page: Page, ext: 'csv' | 'parquet') {
  await page.goto('/')
  const cell = (name: string) => page.getByRole('region', { name })
  await cell('Schedules').locator('input[type=file]').setInputFiles(`${FIX}/synth_small_schedules.${ext}`)
  await expect(cell('Schedules').locator('table')).toBeVisible()
  await cell('Attributes').locator('input[type=file]').setInputFiles(`${FIX}/synth_small_attributes.${ext}`)
  await expect(cell('Attributes').locator('.collist')).toBeVisible()
  // Roles are guessed from the column names.
  await expect(cell('Schedules').getByRole('combobox', { name: 'Person id' })).toHaveValue('pid')
  await expect(cell('Schedules').getByRole('combobox', { name: 'Activity' })).toHaveValue('act')
  await cell('Schedules').getByRole('combobox', { name: 'Order (ties)' }).selectOption('seq')
  await page.getByRole('button', { name: /^Load/ }).click()
  await expect(page.locator('.window .count b').first()).toBeVisible({ timeout: 30_000 })
}

async function waitIdle(page: Page) {
  await expect(page.locator('.window .bar')).toHaveCount(0, { timeout: 30_000 })
}

async function addSort(page: Page, col: string) {
  await page.getByText('+ sort').click()
  await page.keyboard.type(col)
  await page.keyboard.press('Enter')
  await waitIdle(page)
}

test('load, filter, sort, two windows', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  await open(page)
  await expect(page.locator('.window .count').first()).toHaveAttribute('title', /2,000 of 2,000 persons/)
  // vite preview sends COOP/COEP, so the threaded engine should load.
  const { isolated, cores } = await page.evaluate(() => ({ isolated: crossOriginIsolated, cores: navigator.hardwareConcurrency }))
  expect(isolated).toBe(true)
  if (cores > 1) {
    await page.getByTitle('Dataset summary').click()
    await expect(page.getByRole('dialog', { name: 'Dataset summary' })).toContainText(/\d+ threads/)
    await page.keyboard.press('Escape')
    await page.mouse.click(700, 5)
  }

  const w1 = page.locator('.window').nth(0)
  // Filter: employment in {ft-employed, pt-employed}.
  await w1.getByText('+ filter').click()
  await page.keyboard.type('employment')
  await page.keyboard.press('Enter')
  await page.getByLabel('ft-employed').check()
  await page.getByLabel('pt-employed').check()
  await page.getByRole('button', { name: 'Add filter' }).click()
  await waitIdle(page)
  const employed = await w1.locator('.count').getAttribute('title')
  expect(employed).toMatch(/^[\d,]+ of 2,000 persons/)

  // Sort by age then income (shared by all windows).
  for (const col of ['age', 'hh_income']) await addSort(page, col)
  await expect(page.locator('.controls .chip.sort')).toHaveCount(2)

  // Second window: clone, then negate the filter.
  await w1.getByTitle('Clone window').click()
  const w2 = page.locator('.window').nth(1)
  await expect(w2).toBeVisible()
  await w2.locator('.chip.filter .text').first().click()
  await page.getByText('Exclude matches (NOT)').click()
  await page.getByRole('button', { name: 'Update' }).click()
  await waitIdle(page)
  await expect(w1.locator('.title')).toHaveValue('Unnamed_0')
  await expect(w2.locator('.title')).toHaveValue('Unnamed_1')

  const n1 = Number((await w1.locator('.count').getAttribute('title'))!.split(' ')[0].replace(/,/g, ''))
  const n2 = Number((await w2.locator('.count').getAttribute('title'))!.split(' ')[0].replace(/,/g, ''))
  expect(n1 + n2).toBe(2000)

  // Hover shows the activity mix in both windows.
  const box = (await w1.locator('canvas.overlay').boundingBox())!
  await page.mouse.move(box.x + box.width * 0.45, box.y + box.height / 2)
  await expect(page.locator('.wrap .mix')).toHaveCount(2)
  await expect(page.locator('.wrap .line')).toHaveCount(2)
  // Zoomed out, hover snaps to a person in that pixel row: details show in this window only.
  await expect(w1.locator('.tip')).toBeVisible()
  await expect(w2.locator('.tip')).toHaveCount(0)
  await expect(w1.locator('.main .mix')).toHaveCount(0)

  await page.screenshot({ path: 'test-results/smoke.png' })
  expect(errors.join('\n')).toBe('')
})

test('random subset loads a fraction of persons', async ({ page }) => {
  await open(page, '0.25')
  const title = (await page.locator('.window .count').first().getAttribute('title'))!
  const n = Number(title.split(' ')[0].replace(/,/g, ''))
  expect(n).toBeGreaterThan(350)
  expect(n).toBeLessThan(650)
  // Columns load through the subset path too.
  await addSort(page, 'age')
  await expect(page.locator('.chip.sort')).toHaveCount(1)
})

test('view config survives a reload via the URL hash', async ({ page }) => {
  await open(page)
  await addSort(page, 'age')
  await page.getByLabel('Sample', { exact: true }).fill('300')
  await page.getByLabel('Sample', { exact: true }).press('Enter')
  await waitIdle(page)
  // The hash is written shortly after each change: wait for this one.
  const saved = () => {
    const m = /#view=([\w-]+)/.exec(page.url())
    return m ? Buffer.from(m[1], 'base64url').toString() : ''
  }
  await expect.poll(saved).toContain('"n":300')
  expect(saved()).toContain('"column":"age"')
  const url = page.url()
  await page.goto('about:blank')
  await page.goto(url)
  await page.locator('input[accept=".overact"]').setInputFiles(SMALL)
  await expect(page.locator('.controls .chip.sort')).toHaveCount(1)
  await expect(page.getByLabel('Sample', { exact: true })).toHaveValue('300')
  await expect(page.locator('.window .count b').first()).toHaveText('300')
})

test('sample, show, crop and reordering sort keys', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  await open(page)
  const w1 = page.locator('.window').nth(0)

  // Sample after the filter: the count drops, the matching total stays.
  await page.getByLabel('Sample', { exact: true }).fill('100')
  await page.getByLabel('Sample', { exact: true }).press('Enter')
  await waitIdle(page)
  await expect(w1.locator('.count')).toHaveAttribute('title', /^100 sampled from 2,000 matching/)

  // Show a few rows; scrolling moves through them.
  await page.getByLabel('Show', { exact: true }).fill('5')
  await page.getByLabel('Show', { exact: true }).press('Enter')
  await expect(w1.locator('.thumb')).toBeVisible()
  const top0 = await w1.locator('.thumb').evaluate((el) => el.offsetTop)
  const box = (await w1.locator('canvas.overlay').boundingBox())!
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2)
  await page.mouse.wheel(0, 400)
  await expect.poll(() => w1.locator('.thumb').evaluate((el) => el.offsetTop)).toBeGreaterThan(top0)

  // Crop the time axis.
  await page.getByLabel('Crop start').fill('06:00')
  await page.getByLabel('Crop start').press('Enter')
  await page.getByLabel('Crop end').fill('22:00')
  await page.getByLabel('Crop end').press('Enter')
  await expect(page.getByRole('checkbox', { name: 'Crop' })).toBeChecked()
  await expect(w1.locator('.axis .tick').first()).toContainText('06:00')

  // Drag the second sort key in front of the first.
  await addSort(page, 'age')
  await addSort(page, 'hh_income')
  const chips = page.locator('.controls .chip.sort')
  await expect(chips.nth(0)).toContainText('age')
  const a = (await chips.nth(0).boundingBox())!
  const b = (await chips.nth(1).boundingBox())!
  await page.mouse.move(b.x + 20, b.y + b.height / 2)
  await page.mouse.down()
  await page.mouse.move(b.x + 10, b.y + b.height / 2, { steps: 3 })
  await page.mouse.move(a.x + 5, a.y + a.height / 2, { steps: 8 })
  await page.mouse.up()
  await expect(chips.nth(0)).toContainText('hh_income')
  await expect(chips.nth(1)).toContainText('age')
  await waitIdle(page)

  // A plain click on a key still flips its direction.
  await chips.nth(1).getByText('age').click()
  await expect(chips.nth(1)).toContainText('age ↓')
  await expect(chips.nth(0)).toContainText('hh_income ↑')
  await waitIdle(page)

  // Dragging any strip divider resizes the strips of all windows.
  await w1.getByTitle('Clone window').click()
  await waitIdle(page)
  const strips = page.locator('canvas.strip')
  await expect(strips).toHaveCount(2)
  const h0 = (await strips.nth(1).boundingBox())!.height
  const d = (await page.locator('.divider').nth(1).boundingBox())!
  await page.mouse.move(d.x + d.width / 2, d.y + d.height / 2)
  await page.mouse.down()
  await page.mouse.move(d.x + d.width / 2, d.y + d.height / 2 - 40, { steps: 4 })
  await page.mouse.up()
  for (const i of [0, 1]) await expect.poll(async () => (await strips.nth(i).boundingBox())!.height).toBeCloseTo(h0 - 40, 0)
  await page.screenshot({ path: 'test-results/controls.png' })
  expect(errors.join('\n')).toBe('')
})

test('drag windows to rearrange them', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  await open(page)
  await page.getByRole('button', { name: 'Window', exact: true }).click()
  await page.getByRole('button', { name: 'Window', exact: true }).click()
  await waitIdle(page)
  const titles = () => page.locator('.window .title').evaluateAll((els) => els.map((e) => (e as HTMLInputElement).value))
  expect(await titles()).toEqual(['Unnamed_0', 'Unnamed_1', 'Unnamed_2'])
  const grip = (await page.locator('.window').nth(0).getByLabel('Move window').boundingBox())!
  const last = (await page.locator('.window').nth(2).boundingBox())!
  await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2)
  await page.mouse.down()
  await page.mouse.move(last.x + last.width / 2, last.y + 40, { steps: 10 })
  await page.mouse.up()
  expect(await titles()).toEqual(['Unnamed_1', 'Unnamed_2', 'Unnamed_0'])
  // Moved windows still draw (their canvases belong to the worker).
  // Every window sorts (all three ask for the "age" column while it loads).
  await addSort(page, 'age')
  const gutters = () =>
    page.locator('.gutter canvas').evaluateAll((els) => new Set(els.map((e) => (e as HTMLCanvasElement).toDataURL())).size)
  await expect.poll(gutters).toBe(1)
  await page.screenshot({ path: 'test-results/moved.png' })
  expect(errors.join('\n')).toBe('')
})

test('old default titles in saved views are renamed', async ({ page }) => {
  const cfg = {
    overact: 1,
    dataset: { name: 'x', persons: 2000, fingerprint: '' },
    time: { t0: 0, t1: 1440 },
    colors: {},
    hidden: [],
    vertical: { mode: 'fit', f0: 0, f1: 1, r0: 0, rpp: 1 },
    renderMode: 0,
    maxSamples: 32,
    linkSort: true,
    sharedSort: [],
    windows: [
      { title: 'All persons', filter: [], sort: [] },
      { title: 'Workers', filter: [], sort: [] },
    ],
  }
  const hash = Buffer.from(JSON.stringify(cfg)).toString('base64url')
  await page.goto(`/#view=${hash}`)
  await page.locator('input[accept=".overact"]').setInputFiles(SMALL)
  await expect(page.locator('.window .title').nth(1)).toHaveValue('Workers')
  await expect(page.locator('.window .title').nth(0)).toHaveValue('Unnamed_0')
})

test('sort and filter by nested schedule measures', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  await open(page)
  // Sort: end of the 1st episode of any activity (when people first leave home).
  await page.getByText('+ sort').click()
  await page.getByRole('radio', { name: 'End' }).click()
  await page.getByLabel('Which episode').selectOption({ label: 'the 1st episode' })
  await page.getByRole('button', { name: 'Sort', exact: true }).click()
  await waitIdle(page)
  await expect(page.locator('.controls .chip.sort')).toContainText('end of 1st episode ↑')

  // Double-click a key to edit it: the picker opens on that measure, and the
  // key is replaced in place, keeping its direction.
  await addSort(page, 'age')
  const chips = page.locator('.controls .chip.sort')
  await chips.nth(0).getByText('end of 1st episode').click()
  await expect(chips.nth(0)).toContainText('end of 1st episode ↓')
  await waitIdle(page)
  await chips.nth(0).getByText('end of 1st episode').dblclick()
  await expect(page.getByRole('radio', { name: 'End' })).toHaveAttribute('aria-checked', 'true')
  await expect(page.getByLabel('Which episode')).toHaveValue('1')
  await page.getByLabel('Which episode').selectOption({ label: 'the 2nd episode' })
  await page.getByRole('button', { name: 'Update', exact: true }).click()
  await waitIdle(page)
  await expect(chips).toHaveCount(2)
  await expect(chips.nth(0)).toContainText('end of 2nd episode ↓')
  await expect(chips.nth(1)).toContainText('age ↑')
  // Editing an attribute key highlights it in the list; picking another replaces it.
  await chips.nth(1).getByText('age').dblclick()
  await expect(page.locator('.picker .item.active')).toContainText('age')
  await page.locator('.picker .item', { hasText: 'hh_income' }).click()
  await waitIdle(page)
  await expect(chips.nth(1)).toContainText('hh_income ↑')
  await chips.nth(0).getByText('end of 2nd episode').dblclick()
  await page.keyboard.press('Escape')
  await expect(chips.nth(0)).toContainText('end of 2nd episode ↓')
  // Filter: schedules that contain work, in one step.
  const w1 = page.locator('.window').nth(0)
  const count = async () => Number((await w1.locator('.count').getAttribute('title'))!.split(' ')[0].replace(/,/g, ''))
  await w1.getByText('+ filter').click()
  await page.getByLabel('work', { exact: true }).check()
  await page.getByRole('button', { name: 'Filter', exact: true }).click()
  await waitIdle(page)
  await expect(w1.locator('.chip.filter')).toHaveText(/contains work/)
  const withWork = await count()
  expect(withWork).toBeGreaterThan(0)
  expect(withWork).toBeLessThan(2000)
  // Several boxes at once: all must hold (one chip each).
  await w1.locator('.chip.filter .x').click()
  await w1.getByText('+ filter').click()
  await page.getByLabel('work', { exact: true }).check()
  await page.getByLabel('shop', { exact: true }).check()
  await expect(page.locator('.picker .preview')).toHaveText('contains work and contains shop')
  await page.getByRole('button', { name: 'Filter', exact: true }).click()
  await waitIdle(page)
  await expect(w1.locator('.chip.filter')).toHaveCount(2)
  expect(await count()).toBeLessThan(withWork)
  await w1.locator('.chip.filter .x').first().click()
  await w1.locator('.chip.filter .x').first().click()
  await waitIdle(page)
  // Contains something other than home (left the house), negated in the editor.
  await w1.getByText('+ filter').click()
  await page.getByLabel('not home', { exact: true }).check()
  await page.getByRole('button', { name: 'Filter', exact: true }).click()
  await waitIdle(page)
  await expect(w1.locator('.chip.filter')).toHaveText(/contains non-home/)
  const out = await count()
  await w1.locator('.chip.filter .text').click()
  await page.getByText('Exclude matches (NOT)').click()
  await page.getByRole('button', { name: 'Update' }).click()
  await waitIdle(page)
  await expect(w1.locator('.chip.filter')).toHaveText(/doesn't contain non-home/)
  expect(out + (await count())).toBe(2000)
  await w1.locator('.chip.filter .x').click()
  await waitIdle(page)

  // Doing: home throughout 09:00-17:00, and its complement via "not home".
  const doing = async (box: string) => {
    await w1.getByText('+ filter').click()
    await page.getByRole('radio', { name: 'Doing' }).click()
    await page.getByLabel('When').selectOption({ label: box.startsWith('not') ? 'at any time during' : 'throughout' })
    await page.getByLabel(box, { exact: true }).check()
    await page.getByRole('button', { name: 'Filter', exact: true }).click()
    await waitIdle(page)
    const n = await count()
    await w1.locator('.chip.filter .x').click()
    await waitIdle(page)
    return n
  }
  const homeAllDay = await doing('home')
  const leftHome = await doing('not home')
  expect(homeAllDay + leftHome).toBe(2000)

  await w1.getByText('+ filter').click()
  await expect(page.getByRole('radio')).toHaveText(['Contains', 'Doing'])
  await page.keyboard.press('Escape')
  await page.mouse.click(5, 5)

  // Sorting can count different activities too.
  await page.getByText('+ sort').click()
  await page.getByRole('radio', { name: 'Count' }).click()
  await page.getByLabel('What to count').selectOption({ label: 'different activities' })
  await page.getByRole('button', { name: 'Sort', exact: true }).click()
  await waitIdle(page)
  await expect(page.locator('.controls .chip.sort').last()).toContainText('distinct activities')
  await page.screenshot({ path: 'test-results/measures.png' })
  expect(errors.join('\n')).toBe('')
})

for (const ext of ['csv', 'parquet'] as const) {
  test(`load ${ext} tables directly`, async ({ page }) => {
    const errors: string[] = []
    page.on('pageerror', (e) => errors.push(e.message))
    await openTables(page, ext)
    await expect(page.locator('.window .count').first()).toHaveAttribute('title', /2,000 of 2,000 persons/)
    // Same dataset as the packed fixture: same segments, activity order and columns.
    const summary = async () => {
      await page.getByTitle('Dataset summary').click()
      const info = page.getByRole('dialog', { name: 'Dataset summary' })
      const rows = await info.locator('tr').filter({ hasText: /^(Segments|Time axis)/ }).allInnerTexts()
      const text = [rows, await info.locator('p').first().innerText(), await info.locator('.cols').innerText()]
      await page.keyboard.press('Escape')
      await page.mouse.click(5, 5)
      return text
    }
    const direct = await summary()
    await open(page)
    expect(direct).toEqual(await summary())
    expect(errors.join('\n')).toBe('')
  })
}

test('save tables as .overact and reopen', async ({ page }) => {
  await openTables(page, 'parquet')
  await page.getByTitle('View, export and display options').click()
  const [dl] = await Promise.all([page.waitForEvent('download'), page.getByRole('menuitem', { name: 'Dataset as .overact file' }).click()])
  expect(dl.suggestedFilename()).toBe('synth_small_schedules.overact')
  const file = await dl.path()
  await page.goto('/')
  await page.locator('input[accept=".overact"]').setInputFiles({ name: 'saved.overact', mimeType: 'application/octet-stream', buffer: (await import('node:fs')).readFileSync(file) })
  await expect(page.locator('.window .count').first()).toHaveAttribute('title', /2,000 of 2,000 persons/)
})

test('demo opens sorted; up to six windows', async ({ page }) => {
  await page.goto('/')
  await page.getByRole('button', { name: 'Demo dataset' }).click()
  await expect(page.locator('.window .count b').first()).toBeVisible({ timeout: 30_000 })
  await expect(page.locator('.controls .chip.sort')).toHaveCount(1)
  await expect(page.locator('.controls .chip.sort')).toContainText('duration of 1st episode ↑')
  const add = page.getByRole('button', { name: 'Window', exact: true })
  for (let i = 1; i < 6; i++) await add.click()
  await expect(page.locator('.window')).toHaveCount(6)
  await expect(add).toBeDisabled()
})

test('small demo opens with four employment windows', async ({ page }) => {
  await page.goto('/')
  await page.getByRole('button', { name: 'Small demo' }).click()
  await expect(page.locator('.window')).toHaveCount(4, { timeout: 30_000 })
  await expect(page.locator('.controls .chip.sort')).toContainText('duration of 1st episode ↑')
  const counts = page.locator('.window .count')
  await expect(counts.nth(0)).toHaveAttribute('title', /81 of 200 persons/)
  await expect(counts.nth(1)).toHaveAttribute('title', /36 of 200 persons/)
  await expect(counts.nth(2)).toHaveAttribute('title', /35 of 200 persons/)
  await expect(counts.nth(3)).toHaveAttribute('title', /12 of 200 persons/)
})

test('time axis: brush to zoom or filter, wheel and keys zoom', async ({ page }) => {
  await open(page)
  const w1 = page.locator('.window').nth(0)
  const axis = w1.locator('.axis')
  const box = (await axis.boundingBox())!
  const at = (f: number) => box.x + box.width * f
  // Brush 25%..50% of the day (06:00-12:00) and zoom to it.
  await page.mouse.move(at(0.25), box.y + box.height / 2)
  await page.mouse.down()
  await page.mouse.move(at(0.5), box.y + box.height / 2, { steps: 5 })
  await page.mouse.up()
  const pick = page.getByRole('dialog', { name: 'Selected time range' })
  await expect(pick).toContainText(/06:0\d–1[12]:\d\d/)
  await pick.getByRole('button', { name: 'Zoom to this range' }).click()
  await expect(page.getByRole('checkbox', { name: 'Crop' })).toBeChecked()
  await expect(page.getByLabel('Crop start')).toHaveValue(/^06:0\d$/)
  // Whole day again.
  await axis.dblclick()
  await expect(page.getByRole('checkbox', { name: 'Crop' })).not.toBeChecked()

  // Brush again and filter: persons at work at any time 06:00-12:00.
  const before = Number((await w1.locator('.count').getAttribute('title'))!.split(' ')[0].replace(/,/g, ''))
  await page.mouse.move(at(0.25), box.y + box.height / 2)
  await page.mouse.down()
  await page.mouse.move(at(0.5), box.y + box.height / 2, { steps: 5 })
  await page.mouse.up()
  await pick.getByLabel('Activity').selectOption({ label: 'work' })
  await pick.getByRole('button', { name: 'Add' }).click()
  await expect(w1.locator('.chip.filter')).toContainText(/doing work during 06:0\d/)
  await expect
    .poll(async () => Number((await w1.locator('.count').getAttribute('title'))!.split(' ')[0].replace(/,/g, '')))
    .toBeLessThan(before)

  // Keyboard and wheel zoom.
  await page.locator('body').press('+')
  await expect(page.getByRole('checkbox', { name: 'Crop' })).toBeChecked()
  await page.locator('body').press('0')
  await expect(page.getByRole('checkbox', { name: 'Crop' })).not.toBeChecked()
  await page.mouse.move(at(0.5), box.y + box.height / 2)
  await page.mouse.wheel(0, -300)
  await expect(page.getByRole('checkbox', { name: 'Crop' })).toBeChecked()
})

test('compare two datasets', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  await open(page)
  await addSort(page, 'hh_income')
  // Cancel leaves the loaded dataset as it was.
  await page.getByRole('button', { name: 'Dataset', exact: true }).click()
  await page.getByRole('dialog', { name: 'Add a dataset' }).getByRole('button', { name: 'Cancel' }).click()
  await expect(page.locator('.window')).toHaveCount(1)
  // Add a second dataset: a window on it opens beside the first.
  await page.getByRole('button', { name: 'Dataset', exact: true }).click()
  await page.getByRole('button', { name: 'Small demo' }).click()
  await expect(page.locator('.window')).toHaveCount(2, { timeout: 30_000 })
  await waitIdle(page)
  const counts = page.locator('.window .count')
  await expect(counts.nth(0)).toHaveAttribute('title', /2,000 of 2,000 persons in synth_small/)
  await expect(counts.nth(1)).toHaveAttribute('title', /200 of 200 persons in demo-small/)
  // The legend has the activities of both; the sort key only one has is marked.
  await expect(page.locator('.legendbar')).toContainText('travel')
  await expect(page.locator('.chip.sort.partial')).toHaveCount(1)
  // Switch the first window to the other dataset, then back.
  const pick = page.locator('.window').nth(0).getByRole('combobox', { name: 'Dataset' })
  await pick.selectOption({ label: 'demo-small.overact' })
  await expect(counts.nth(0)).toHaveAttribute('title', /200 of 200 persons in demo-small/)
  await page.locator('.window').nth(0).getByRole('combobox', { name: 'Dataset' }).selectOption({ label: 'synth_small.overact' })
  await expect(counts.nth(0)).toHaveAttribute('title', /2,000 of 2,000 persons in synth_small/)
  // Saved views name the datasets.
  await expect.poll(() => Buffer.from(/#view=([\w-]+)/.exec(page.url())?.[1] ?? '', 'base64url').toString()).toContain('"datasets"')
  // Removing a dataset closes its windows.
  await page.getByTitle('Dataset summary').click()
  const info = page.getByRole('dialog', { name: 'Dataset summary' })
  await info.getByRole('tab', { name: 'demo-small.overact' }).click()
  await info.getByRole('button', { name: 'Remove' }).click()
  await expect(page.locator('.window')).toHaveCount(1)
  await expect(page.locator('.chip.sort.partial')).toHaveCount(0)
  expect(errors.join('\n')).toBe('')
})
