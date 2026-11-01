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

/** `#rrggbb` or `#rgb` (as minified CSS has it) as 0xRRGGBB. */
export function hexToInt(hex: string): number {
  let h = hex.trim().replace('#', '')
  if (h.length === 3 || h.length === 4) h = [...h.slice(0, 3)].map((c) => c + c).join('')
  return parseInt(h.slice(0, 6), 16) || 0
}

/** `#rrggbb` as a little-endian RGBA pixel (as `Uint32Array` views of ImageData see it). */
export function pixel(hex: string): number {
  const c = hexToInt(hex)
  return (0xff000000 | ((c & 0xff) << 16) | (c & 0xff00) | ((c >> 16) & 0xff)) >>> 0
}

/** `#rrggbb` part way (t in [0, 1]) from colour `a` to colour `b`, in sRGB. */
export function mix(a: string, b: string, t: number): string {
  const x = hexToInt(a)
  const y = hexToInt(b)
  const ch = (s: number) => Math.round(((x >> s) & 255) * (1 - t) + ((y >> s) & 255) * t)
  return '#' + ((ch(16) << 16) | (ch(8) << 8) | ch(0)).toString(16).padStart(6, '0')
}

const LINEAR = Array.from({ length: 256 }, (_, i) => {
  const c = i / 255
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
})

/** `#rrggbb` as linear-light RGB (0..1), for averaging colours. */
export function toLinear(hex: string): [number, number, number] {
  const c = hexToInt(hex)
  return [LINEAR[(c >> 16) & 255], LINEAR[(c >> 8) & 255], LINEAR[c & 255]]
}

/** A linear-light channel (0..1) as an sRGB byte. */
export function fromLinear(x: number): number {
  const c = x <= 0.0031308 ? x * 12.92 : 1.055 * x ** (1 / 2.4) - 0.055
  return Math.max(0, Math.min(255, Math.round(c * 255)))
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
