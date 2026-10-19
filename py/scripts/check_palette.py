"""Colour-vision-deficiency check for the default activity palette.

Simulates protanopia, deuteranopia and tritanopia (Machado, Oliveira &
Fernandes 2009, severity 1.0) and reports the smallest CIEDE2000 distance
between any two activity colours, plus the closest pair. Run:

    uv run --project py python py/scripts/check_palette.py

The palette is read from web/src/lib/palette.ts so the check cannot drift.
"""

import itertools
import re
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]

MACHADO = {
    "normal": np.eye(3),
    "protanopia": np.array(
        [
            [0.152286, 1.052583, -0.204868],
            [0.114503, 0.786281, 0.099216],
            [-0.003882, -0.048116, 1.051998],
        ]
    ),
    "deuteranopia": np.array(
        [
            [0.367322, 0.860646, -0.227968],
            [0.280085, 0.672501, 0.047413],
            [-0.011820, 0.042940, 0.968881],
        ]
    ),
    "tritanopia": np.array(
        [
            [1.255528, -0.076749, -0.178779],
            [-0.078411, 0.930809, 0.147602],
            [0.004733, 0.691367, 0.303900],
        ]
    ),
}


def srgb_to_linear(c):
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def linear_to_lab(rgb):
    m = np.array([[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]])
    xyz = rgb @ m.T / np.array([0.95047, 1.0, 1.08883])
    f = np.where(xyz > (6 / 29) ** 3, np.cbrt(xyz), xyz / (3 * (6 / 29) ** 2) + 4 / 29)
    return np.stack([116 * f[:, 1] - 16, 500 * (f[:, 0] - f[:, 1]), 200 * (f[:, 1] - f[:, 2])], 1)


def ciede2000(a, b):
    L1, a1, b1 = a
    L2, a2, b2 = b
    C1, C2 = np.hypot(a1, b1), np.hypot(a2, b2)
    Cb = (C1 + C2) / 2
    G = 0.5 * (1 - np.sqrt(Cb**7 / (Cb**7 + 25**7)))
    a1p, a2p = (1 + G) * a1, (1 + G) * a2
    C1p, C2p = np.hypot(a1p, b1), np.hypot(a2p, b2)
    h1p = np.degrees(np.arctan2(b1, a1p)) % 360
    h2p = np.degrees(np.arctan2(b2, a2p)) % 360
    dLp, dCp = L2 - L1, C2p - C1p
    dh = h2p - h1p
    if C1p * C2p == 0:
        dh = 0
    elif dh > 180:
        dh -= 360
    elif dh < -180:
        dh += 360
    dHp = 2 * np.sqrt(C1p * C2p) * np.sin(np.radians(dh / 2))
    Lbp, Cbp = (L1 + L2) / 2, (C1p + C2p) / 2
    if C1p * C2p == 0:
        hbp = h1p + h2p
    elif abs(h1p - h2p) <= 180:
        hbp = (h1p + h2p) / 2
    else:
        hbp = (h1p + h2p + 360) / 2 if h1p + h2p < 360 else (h1p + h2p - 360) / 2
    T = (
        1
        - 0.17 * np.cos(np.radians(hbp - 30))
        + 0.24 * np.cos(np.radians(2 * hbp))
        + 0.32 * np.cos(np.radians(3 * hbp + 6))
        - 0.20 * np.cos(np.radians(4 * hbp - 63))
    )
    dtheta = 30 * np.exp(-(((hbp - 275) / 25) ** 2))
    Rc = 2 * np.sqrt(Cbp**7 / (Cbp**7 + 25**7))
    Sl = 1 + 0.015 * (Lbp - 50) ** 2 / np.sqrt(20 + (Lbp - 50) ** 2)
    Sc, Sh = 1 + 0.045 * Cbp, 1 + 0.015 * Cbp * T
    Rt = -np.sin(np.radians(2 * dtheta)) * Rc
    return np.sqrt(
        (dLp / Sl) ** 2 + (dCp / Sc) ** 2 + (dHp / Sh) ** 2 + Rt * (dCp / Sc) * (dHp / Sh)
    )


def main() -> None:
    src = (ROOT / "web/src/lib/palette.ts").read_text()
    named = dict(
        re.findall(r"^\s+(\w+): '(#[0-9a-f]{6})'", src.split("const CYCLE")[0], re.MULTILINE)
    )
    # The synthetic demo's ten activities plus 'unknown'.
    labels = [
        "home",
        "work",
        "education",
        "travel",
        "shop",
        "leisure",
        "visit",
        "escort",
        "medical",
        "other",
        "unknown",
    ]
    hexes = [named[l] for l in labels]
    rgb = np.array([[int(h[i : i + 2], 16) / 255 for i in (1, 3, 5)] for h in hexes])
    lin = srgb_to_linear(rgb)
    print("| vision | min ΔE2000 | closest pair |\n|---|---|---|")
    for name, m in MACHADO.items():
        lab = linear_to_lab(np.clip(lin @ m.T, 0, 1))
        best = min(
            (
                (ciede2000(lab[i], lab[j]), labels[i], labels[j])
                for i, j in itertools.combinations(range(len(labels)), 2)
            ),
            key=lambda t: t[0],
        )
        print(f"| {name} | {best[0]:.1f} | {best[1]} / {best[2]} |")


if __name__ == "__main__":
    main()
