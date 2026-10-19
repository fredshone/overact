"""Assemble README GIFs from frames written by web/scripts/capture-media.mjs.

Frame files are named <sequence>-<index>-<hold>.png; <hold> repeats the frame.
Usage: uv run --project py --with pillow python py/scripts/make_gif.py <framedir> [outdir]
"""

import sys
from collections import defaultdict
from pathlib import Path

from PIL import Image


def main() -> None:
    src = Path(sys.argv[1])
    out = (
        Path(sys.argv[2])
        if len(sys.argv) > 2
        else Path(__file__).resolve().parents[2] / "docs/media"
    )
    out.mkdir(parents=True, exist_ok=True)
    seqs: dict[str, list[tuple[int, int, Path]]] = defaultdict(list)
    for p in src.glob("*.png"):
        name, idx, hold = p.stem.rsplit("-", 2)
        seqs[name].append((int(idx), int(hold), p))
    for name, frames in seqs.items():
        frames.sort()
        imgs, durations = [], []
        for _, hold, p in frames:
            im = Image.open(p).convert("RGB")
            im = im.resize((im.width * 3 // 4, im.height * 3 // 4), Image.LANCZOS)
            imgs.append(
                im.quantize(colors=128, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
            )
            durations.append(120 * hold)
        target = out / f"{name}.gif"
        imgs[0].save(
            target, save_all=True, append_images=imgs[1:], duration=durations, loop=0, optimize=True
        )
        print(f"{target}: {len(imgs)} frames, {target.stat().st_size / 1e6:.1f} MB")


if __name__ == "__main__":
    main()
