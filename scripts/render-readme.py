#!/usr/bin/env python3
"""Render the README GIF from real Ratatui buffers. Requires Python 3 and Pillow.

Run from anywhere: python3 scripts/render-readme.py
Builds the existing workspace example, replays keys against demo fixtures, and
writes docs/media/conductor.gif. No terminal capture, credentials or agent calls.
"""
import argparse
import json
import math
import re
import shutil
import subprocess
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1]
WIDTH, HEIGHT = 108, 34
FONT_SIZE, CELL_HEIGHT = 14, 20
BACKGROUND = (23, 27, 32)
TEXT = (216, 223, 232)

# Scene, actual keys since opening it, and time to read the resulting screen.
TOUR = [
    ("series", "", 2000),
    ("series", "down", 650),
    ("series", "down,down", 650),
    ("series", "down,down,down", 650),
    ("series", "down,down,down,enter", 2800),
    ("series", "down,down,down,enter,esc,b", 1800),
    ("bug", "", 2600),
    ("bug", "down", 550),
    ("bug", "down,down", 550),
    ("bug", "down,down,enter", 2600),
    ("bug", "p", 1500),
    ("bug", "p,right", 1500),
    ("clarify", "", 2000),
    ("clarify", "down", 700),
    ("clarify", "down,up,space", 1200),
    ("picker", "", 1500),
    ("picker", "down,down", 1000),
    ("picker", "down,down,down,tab", 2000),
]


def fonts(path, bold_path):
    if path is None:
        candidates = [
            Path("/System/Library/Fonts/Menlo.ttc"),
            Path("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"),
        ]
        path = next((p for p in candidates if p.exists()), None)
        if path is None:
            raise SystemExit("Pass --font /path/to/a/monospace.ttf (optionally --bold-font).")
    regular = ImageFont.truetype(str(path), FONT_SIZE)
    bold = regular
    if bold_path:
        bold = ImageFont.truetype(str(bold_path), FONT_SIZE)
    elif path.suffix.lower() == ".ttc":
        for index in range(1, 8):
            try:
                candidate = ImageFont.truetype(str(path), FONT_SIZE, index=index)
            except OSError:
                break
            if candidate.getname()[1] == "Bold":
                bold = candidate
                break
    else:
        candidate = path.with_name(path.stem + "-Bold.ttf")
        if candidate.exists():
            bold = ImageFont.truetype(str(candidate), FONT_SIZE)
    return regular, bold


def color(value, default):
    match = re.fullmatch(r"Rgb\((\d+), (\d+), (\d+)\)", value)
    return tuple(map(int, match.groups())) if match else default


def render(data, regular, bold):
    cell_width = math.ceil(regular.getlength("M"))
    canvas = Image.new("RGB", (data["width"] * cell_width, data["height"] * CELL_HEIGHT), BACKGROUND)
    draw = ImageDraw.Draw(canvas)
    # Paint every cell before text; adjacent backgrounds must not clip glyphs.
    for y, row in enumerate(data["rows"]):
        for x, cell in enumerate(row):
            draw.rectangle((x * cell_width, y * CELL_HEIGHT, (x + 1) * cell_width - 1,
                            (y + 1) * CELL_HEIGHT - 1), fill=color(cell["bg"], BACKGROUND))
    for y, row in enumerate(data["rows"]):
        for x, cell in enumerate(row):
            left, top = x * cell_width, y * CELL_HEIGHT
            right, bottom = left + cell_width - 1, top + CELL_HEIGHT - 1
            symbol, foreground = cell["text"], color(cell["fg"], TEXT)
            # Terminals join box glyphs and fill block glyphs to cell boundaries.
            # Font metrics alone can leave gaps or shift a half-block chart.
            if symbol in ("█", "▀", "▄"):
                start = top + CELL_HEIGHT // 2 if symbol == "▄" else top
                end = top + CELL_HEIGHT // 2 - 1 if symbol == "▀" else bottom
                draw.rectangle((left, start, right, end), fill=foreground)
                continue
            connections = {
                "─": "lr", "│": "ud", "┌": "rd", "┐": "ld",
                "└": "ru", "┘": "lu", "├": "urd", "┤": "uld",
                "┬": "lrd", "┴": "lru", "┼": "lrud",
            }.get(symbol)
            if connections:
                center = (left + cell_width // 2, top + CELL_HEIGHT // 2)
                ends = {"l": (left, center[1]), "r": (right, center[1]),
                        "u": (center[0], top), "d": (center[0], bottom)}
                for direction in connections:
                    draw.line((center, ends[direction]), fill=foreground)
                continue
            draw.text((left, top), symbol,
                      font=bold if "BOLD" in cell["modifiers"] else regular,
                      fill=foreground)
    return canvas


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--font", type=Path)
    parser.add_argument("--bold-font", type=Path)
    parser.add_argument("--preview-dir", type=Path, help="Also save individual PNGs for visual review")
    args = parser.parse_args()
    regular, bold = fonts(args.font, args.bold_font)
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    build = subprocess.run(
        [cargo, "build", "--locked", "-p", "conductor-tui", "--example", "workspace", "--message-format=json"],
        cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True,
    )
    artifacts = [json.loads(line) for line in build.stdout.splitlines()]
    binary = next(Path(item["executable"]) for item in artifacts
                  if item.get("reason") == "compiler-artifact" and item.get("executable")
                  and item.get("target", {}).get("name") == "workspace")
    frames = []
    if args.preview_dir:
        args.preview_dir.mkdir(parents=True, exist_ok=True)
    for index, (scene, keys, _) in enumerate(TOUR):
        command = [str(binary), str(WIDTH), str(HEIGHT), scene, "--json"]
        if keys:
            command.extend(["--keys", keys])
        data = json.loads(subprocess.check_output(command, cwd=ROOT))
        frame = render(data, regular, bold)
        frames.append(frame)
        if args.preview_dir:
            frame.save(args.preview_dir / f"{index:02d}-{scene}.png")
    # Share one palette so the background and text do not shift between scenes.
    swatches = Image.new("RGB", (324 * len(frames), 227))
    for index, frame in enumerate(frames):
        swatches.paste(frame.resize((324, 227)), (index * 324, 0))
    palette = swatches.quantize(colors=256)
    indexed = [frame.quantize(palette=palette, dither=Image.Dither.NONE) for frame in frames]
    output = ROOT / "docs/media/conductor.gif"
    indexed[0].save(output, save_all=True, append_images=indexed[1:], loop=0,
                    duration=[step[2] for step in TOUR], optimize=True, disposal=1)
    print(f"Wrote {output.relative_to(ROOT)}: {output.stat().st_size / 1024:.0f} KiB")


if __name__ == "__main__":
    main()
