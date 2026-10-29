#!/usr/bin/env python3
"""Renders the RustCraft app icon (the launcher's ember block mark) to
packaging/icons as PNG, ICO and ICNS. Requires Pillow."""
from pathlib import Path
from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "packaging" / "icons"
S = 1024


def render() -> Image.Image:
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    # Vertical ember gradient, masked to a rounded square (macOS icon grid).
    top, bottom = (0xF2, 0x8A, 0x55), (0xD8, 0x62, 0x2C)
    grad = Image.new("RGBA", (S, S))
    gd = ImageDraw.Draw(grad)
    for y in range(S):
        t = y / (S - 1)
        gd.line([(0, y), (S, y)], fill=tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)) + (255,))
    mask = Image.new("L", (S, S), 0)
    m = 100
    ImageDraw.Draw(mask).rounded_rectangle([m, m, S - m, S - m], radius=200, fill=255)
    img.paste(grad, (0, 0), mask)

    # Inset block: two shaded quadrants and a light outline, composited
    # over the gradient.
    overlay = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(overlay)
    a, b = 300, S - 300
    c = S // 2
    d.rectangle([a + 20, a + 20, c, c], fill=(0, 0, 0, 56))
    d.rectangle([c, c, b - 20, b - 20], fill=(0, 0, 0, 56))
    d.rounded_rectangle([a, a, b, b], radius=36, outline=(255, 255, 255, 150), width=26)
    return Image.alpha_composite(img, overlay)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    img = render()
    img.resize((512, 512), Image.LANCZOS).save(OUT / "rustcraft.png")
    img.save(OUT / "rustcraft.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
    img.save(OUT / "rustcraft.icns")
    print("wrote", ", ".join(p.name for p in sorted(OUT.iterdir())))


if __name__ == "__main__":
    main()
