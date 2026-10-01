# README animation

`conductor.gif` shows actual Conductor/Ratatui rendering with illustrative demo
data. Each demonstrated interaction replays keys through the workspace's input handler. No
agent runs, live forecasts, or real review decisions are represented.

The loop shows chart selection → source inspection → a follow-up draft → changes
and test evidence → clarification choices → the agent picker. It deliberately
stops before sending a question or accepting a review.

To regenerate, install Python 3 with Pillow and the project's Rust toolchain:

```sh
python3 scripts/render-readme.py
```

The renderer uses Menlo on macOS or DejaVu Sans Mono on Linux. For another font,
pass `--font /path/to/monospace.ttf` and optionally `--bold-font`. Fonts are not
bundled. Add `--preview-dir /tmp/conductor-frames` to inspect individual PNGs.

The standalone [still image](../screenshots/workbench-series.png) is available
for readers who prefer no animation. This is documentation tooling; the terminal
application does not use Python or Pillow.
