#!/usr/bin/env python3
"""Convert the approved logo to web, Windows and macOS icon formats.

Run with Python 3 and Pillow installed: python3 scripts/generate-icons.py
"""

from pathlib import Path

from PIL import Image


root = Path(__file__).resolve().parents[1]
branding = root / "assets" / "branding"
public = root / "apps" / "web" / "public"
public.mkdir(parents=True, exist_ok=True)

with Image.open(branding / "lanlink-logo.png") as source:
    logo = source.convert("RGBA")
    if logo.width != logo.height:
        raise ValueError("The approved logo must be square")
    for filename, size in (
        ("logo.png", 512),
        ("favicon-32.png", 32),
        ("apple-touch-icon.png", 180),
    ):
        logo.resize((size, size), Image.Resampling.LANCZOS).save(public / filename)
    logo.save(
        branding / "lanlink.ico",
        sizes=[(size, size) for size in (16, 24, 32, 48, 64, 128, 256)],
    )
    logo.resize((1024, 1024), Image.Resampling.LANCZOS).save(
        branding / "lanlink.icns"
    )

(public / "favicon.ico").write_bytes((branding / "lanlink.ico").read_bytes())
