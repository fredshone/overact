// Captures frame sequences for the README GIFs (run against `npx vite`).
// Usage: node scripts/capture-media.mjs <outdir> <tabledir>
// Sequence 4 opens the two demos from public/demo.
// <tabledir> holds activities.csv and attributes.csv from `overact-synth -n 200000 --format csv`.
// Then: uv run --project ../py --with pillow python ../py/scripts/make_gif.py <outdir>
import { chromium } from 'playwright'
import fs from 'node:fs'
import path from 'node:path'

const out = process.argv[2] ?? '/tmp/overact-frames'
const tables = process.argv[3]
if (!tables) throw new Error('usage: capture-media.mjs <outdir> <tabledir>')
const base = process.env.BASE ?? 'http://localhost:5173'
fs.mkdirSync(out, { recursive: true })
const browser = await chromium.launch()
const page = await browser.newPage({ viewport: { width: 1200, height: 760 }, deviceScaleFactor: 1 })

let n = 0
let seq = ''
async function frame(hold = 1) {
  await page.waitForTimeout(120)
  const p = path.join(out, `${seq}-${String(n++).padStart(3, '0')}-${hold}.png`)
  await page.screenshot({ path: p })
}
const idle = () => page.waitForFunction(() => !document.querySelector('.window .bar'), null, { timeout: 60_000 })

// 1. Load the schedules and attributes tables.
seq = 'load'
n = 0
await page.goto(base)
await frame(8)
const cell = (name) => page.getByRole('region', { name })
await cell('Schedules').locator('input[type=file]').setInputFiles(path.join(tables, 'activities.csv'))
await cell('Schedules').locator('table').waitFor()
await frame(10)
await cell('Attributes').locator('input[type=file]').setInputFiles(path.join(tables, 'attributes.csv'))
await cell('Attributes').locator('.collist').waitFor()
await frame(8)
await page.getByRole('button', { name: /^Load/ }).click()
await page.waitForSelector('.window .count b', { timeout: 120_000 })
await idle()
await frame(16)

// 2. Compare employed and not employed.
seq = 'compare'
n = 0
await frame(8)
const w1 = page.locator('.window').nth(0)
await page.getByText('+ sort').click()
await page.keyboard.type('age', { delay: 60 })
await frame(4)
await page.keyboard.press('Enter')
await idle()
await frame(8)
await w1.getByText('+ filter').click()
await page.keyboard.type('employment', { delay: 50 })
await page.keyboard.press('Enter')
await frame(4)
await page.getByLabel('ft-employed').check()
await page.getByLabel('pt-employed').check()
await frame(6)
await page.getByRole('button', { name: 'Add filter' }).click()
await idle()
await w1.locator('input.title').fill('Employed')
await frame(8)
await w1.getByTitle('Clone window').click()
await idle()
const w2 = page.locator('.window').nth(1)
await w2.locator('.chip.filter .text').first().click()
await page.getByText('Exclude matches (NOT)').click()
await frame(4)
await page.getByRole('button', { name: 'Update' }).click()
await idle()
await w2.locator('input.title').fill('Not employed')
await page.waitForTimeout(1500)
await frame(14)
await page.getByRole('button', { name: 'Majority' }).click()
await page.waitForTimeout(1500)
await frame(14)
const box = (await w1.locator('canvas.overlay').boundingBox())
for (const f of [0.3, 0.38, 0.46, 0.54, 0.62, 0.7]) {
  await page.mouse.move(box.x + box.width * f, box.y + box.height * 0.5)
  await frame(3)
}

// 3. Show fewer rows to see individual schedules, then scroll through them.
seq = 'zoom'
n = 0
await page.getByRole('button', { name: 'Mean' }).click()
await page.mouse.move(box.x + box.width * 0.4, box.y + box.height * 0.4)
await page.waitForTimeout(800)
await frame(6)
const show = page.getByLabel('Show')
for (const v of ['20k', '2k', '200', '40']) {
  await show.fill(v)
  await show.press('Enter')
  await idle()
  await page.waitForTimeout(400)
  await frame(5)
}
await page.mouse.move(box.x + box.width * 0.45, box.y + box.height * 0.42)
for (let i = 0; i < 6; i++) {
  await page.mouse.wheel(0, 120)
  await page.waitForTimeout(250)
  await frame(1)
}
await page.waitForTimeout(1200)
await page.mouse.move(box.x + box.width * 0.45, box.y + box.height * 0.42)
await page.waitForTimeout(600)
await frame(8)
await page.mouse.click(box.x + box.width * 0.45, box.y + box.height * 0.42)
await page.waitForTimeout(600)
await frame(16)

// 4. Compare the two demo datasets: employed persons in each.
seq = 'datasets'
n = 0
await page.goto(base)
await page.getByRole('button', { name: 'Demo dataset' }).click()
await page.waitForSelector('.window .count b', { timeout: 120_000 })
await idle()
await page.waitForTimeout(800)
await frame(8)
const d1 = page.locator('.window').nth(0)
await d1.getByText('+ filter').click()
await page.keyboard.type('employment', { delay: 50 })
await page.keyboard.press('Enter')
await page.getByLabel('ft-employed').check()
await page.getByLabel('pt-employed').check()
await frame(5)
await page.getByRole('button', { name: 'Add filter' }).click()
await idle()
await d1.locator('input.title').fill('Employed')
await page.waitForTimeout(800)
await frame(8)
await page.getByRole('button', { name: 'Dataset', exact: true }).click()
await page.getByRole('dialog', { name: 'Add a dataset' }).waitFor()
await frame(8)
await page.getByRole('button', { name: 'Small demo' }).click()
await page.locator('.window').nth(1).waitFor({ timeout: 60_000 })
await idle()
await page.locator('.window').nth(1).locator('input.title').fill('Employed (small demo)')
await page.locator('.window').nth(1).locator('input.title').blur()
await page.waitForTimeout(1500)
await frame(16)
const dbox = await d1.locator('canvas.overlay').boundingBox()
for (const f of [0.3, 0.38, 0.46, 0.54, 0.62]) {
  await page.mouse.move(dbox.x + dbox.width * f, dbox.y + dbox.height * 0.5)
  await frame(3)
}
await page.mouse.move(dbox.x + dbox.width * 0.62, dbox.y - 40)
await page.waitForTimeout(300)
await frame(10)
await browser.close()
console.log(`frames in ${out}`)
