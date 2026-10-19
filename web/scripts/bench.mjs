// Runs bench.html in headless Chromium against a dev server and prints the table.
// Usage: npm run bench -- [file-url-or-path ...] [--threads=0] [--partB=0]
// Start `npx vite` first, or set BENCH_BASE (default http://localhost:5173).
import { chromium } from 'playwright'
import path from 'node:path'

const base = process.env.BENCH_BASE ?? 'http://localhost:5173'
const args = process.argv.slice(2)
const extra = args.filter((a) => a.startsWith('--')).map((a) => a.slice(2)).join('&')
const files = args.filter((a) => !a.startsWith('--'))
if (!files.length) files.push('/demo/demo.overact')
const browser = await chromium.launch()
for (const f of files) {
  const url = f.startsWith('/') && !f.startsWith('/demo') ? `/@fs${path.resolve(f)}` : f
  const page = await browser.newPage()
  const done = new Promise((resolve) =>
    page.on('console', (m) => {
      const t = m.text()
      if (t.startsWith('BENCH')) resolve(t)
    }),
  )
  await page.goto(`${base}/bench.html?file=${encodeURIComponent(url)}${extra ? `&${extra}` : ''}`)
  const out = await done
  console.log(`\n${f}`)
  if (out.startsWith('BENCH-ERROR')) {
    console.log(out)
    continue
  }
  const rows = JSON.parse(out.slice(6))
  const fmt = (v) => (v === null ? '' : `${Math.round(v)} ms`)
  console.log('| operation | WASM | plain TS | target | notes |\n|---|---|---|---|---|')
  for (const r of rows) console.log(`| ${r.op} | ${fmt(r.wasm)} | ${fmt(r.ts)} | ${r.target} | ${r.notes} |`)
  await page.close()
}
await browser.close()
