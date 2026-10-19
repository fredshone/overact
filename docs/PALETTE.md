# Default palette and colour-vision check

The defaults live in `web/src/lib/palette.ts`. Common activity labels get
fixed colours chosen for meaning as well as contrast:

- `home` is a pale warm neutral, because it fills most of most days and
  should recede.
- `work` is a strong indigo, and `travel` is near-black, because travel
  segments are short and need to stay visible as thin lines.
- Anything else cycles through Paul Tol's colour-blind-safe "muted" scheme.

Colours are stored per activity label, so a colour you pick follows the label
across datasets. Use **reset** in the legend to go back to the defaults.

## Check

`py/scripts/check_palette.py` reads the palette from the source file. It
simulates protanopia, deuteranopia and tritanopia (Machado, Oliveira &
Fernandes 2009, full severity) and reports the smallest CIEDE2000 distance
between any two of the eleven default activity colours (the synthetic demo's
ten activities plus `unknown`):

```
uv run --project py python py/scripts/check_palette.py
```

| vision | min ΔE2000 | closest pair |
|---|---|---|
| normal | 16.1 | shop / other |
| protanopia | 9.8 | leisure / escort |
| deuteranopia | 9.2 | escort / unknown |
| tritanopia | 10.1 | home / shop |

As a rough guide, ΔE2000 above about 10 is easy to tell apart side by side,
and 5 to 10 is distinguishable with attention. The first version of the
palette scored 6.2 under protanopia (escort / unknown). Changing `escort` to
`#2a9d8f` and `unknown` to a bluish grey `#a0a0b4` raised the worst case to 9.2.

With eleven categories, every pair cannot be far apart under full dichromacy.
The viewer therefore also offers per-activity visibility (click a legend
entry; double-click to show only that activity), majority-colour mode, and
the profile strip readout. None of these depend on telling colours apart.
