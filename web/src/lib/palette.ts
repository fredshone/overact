// Default activity colours. Common labels get fixed, meaningful colours;
// anything else cycles through Paul Tol's colour-blind-safe "muted" scheme.
// `docs/PALETTE.md` records the colour-vision-deficiency check.

const NAMED: Record<string, string> = {
  home: '#e6dfcc',
  work: '#332288',
  education: '#117733',
  school: '#117733',
  travel: '#3b3b3b',
  shop: '#ddcc77',
  shopping: '#ddcc77',
  leisure: '#cc6677',
  visit: '#aa4499',
  escort: '#2a9d8f',
  medical: '#88ccee',
  other: '#999933',
  unknown: '#a0a0b4',
}

const CYCLE = ['#cc6677', '#332288', '#ddcc77', '#117733', '#88ccee', '#882255', '#44aa99', '#999933', '#aa4499', '#6699cc', '#dd7788', '#225555']

export function defaultColors(labels: string[]): string[] {
  const used = new Set<string>()
  const out = labels.map((l) => {
    const c = NAMED[l.toLowerCase()]
    if (c) used.add(c)
    return c ?? ''
  })
  let k = 0
  return out.map((c) => {
    if (c) return c
    while (k < CYCLE.length - 1 && used.has(CYCLE[k])) k++
    const pick = CYCLE[k % CYCLE.length]
    used.add(pick)
    k++
    return pick
  })
}

export function hexToInt(hex: string): number {
  return parseInt(hex.replace('#', '').slice(0, 6), 16) || 0
}

/** Sequential ramp (viridis-like) for the sort-key gutter; t in [0, 1]. */
const RAMP = ['#440154', '#3b528b', '#21918c', '#5ec962', '#fde725']
export function ramp(t: number): string {
  if (!Number.isFinite(t)) return 'transparent'
  const x = Math.min(1, Math.max(0, t)) * (RAMP.length - 1)
  const i = Math.min(RAMP.length - 2, Math.floor(x))
  const f = x - i
  const a = hexToInt(RAMP[i])
  const b = hexToInt(RAMP[i + 1])
  const ch = (s: number) => Math.round(((a >> s) & 255) * (1 - f) + ((b >> s) & 255) * f)
  return `rgb(${ch(16)},${ch(8)},${ch(0)})`
}
