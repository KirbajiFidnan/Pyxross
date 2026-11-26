# Pyxross Workshop v0.9.4 Release Notes

## Downloads

Prebuilt portable binaries for **v0.9.4**:

- **Linux AppImage (x86_64, GLIBC 2.35+):** `pyxross-0.9.4-linux-x86_64.AppImage`
- **Linux Portable Archive (x86_64, GLIBC 2.35+):** `pyxross-0.9.4-linux-x86_64.tar.gz`
- **Windows Portable ZIP (x86_64, Windows 10/11):** `pyxross-0.9.4-windows-x86_64-portable.zip`

## What's New in 0.9.4

- **Canvas-Locked Brush Patterns (Pencil & Eraser):** Added a new `Pattern` selector in the Tool Property panel with three modes — **Normal**, **Bayer Dither**, and **Line** — evaluated strictly in absolute canvas pixel coordinates `(x, y)` so alignment and symmetry stay seamless across separate strokes, `Shift+Line` strokes, and Curve transforms. Pencil and Eraser each remember their own independent pattern settings:
  - **Bayer Dither:** Ordered 8×8 Bayer dithering with a `Density` (`0..=100%`) slider (`50%` produces an exact 1×1 checkerboard, `0%` paints nothing, `100%` paints solid).
  - **Line:** Evenly spaced parallel stripes with `Thickness` (`1..=32` px), `Angle` (`0..=180°`), and `Spacing` (`1..=64` px) sliders.
- **Scrollable Tool Property Panel:** Added vertical scrolling to the Tool Property panel so expanded pattern controls remain cleanly accessible on smaller screens and compact dock layouts.

## Performance & Fixes

- **Large Brush Line Lag Fix (Pencil & Eraser):** Eliminated severe frame drops when drawing or previewing lines with large brushes (up to `64×64` px):
  - Replaced per-stamp `Vec` allocations in `Stroke` with stack-allocated `BrushRowSpans` and delta leading-edge stepping along adjacent Bresenham steps.
  - Replaced per-pixel `BTreeSet` footprint union and per-cell `painter.rect_filled` preview loops with a word-level 1-bit/pixel `FootprintMask`, viewport clipping, horizontal span merging into a single `egui::Mesh`, cached nearest-neighbor texture quads for dense pattern previews, and collinear-merged Eraser outlines.
  - Pattern evaluation uses precomputed O(1) integer/fixed-point lookups with zero per-pixel allocations, keeping both Normal and Pattern painting smooth even at maximum brush size.

## Package Contents

Each package contains:
- Release binary (`pyxross` or `pyxross.exe`)
- `README.md`, MIT `LICENSE`, and `LICENSE-FONT-OFL.txt` (SIL OFL 1.1 for embedded JetBrains Mono Nerd Font)
- External default theme at `themes/builtin/theme.json` and `themes/builtin/atlas.png`
- Built-in top-bar banners in `banners/`
- Linux AppImage additionally includes `AppRun`, `pyxross.desktop`, and `pyxross.png`

## Platform Notes

- Linux builds target `x86_64-unknown-linux-gnu` on an Ubuntu 22.04 (`GLIBC 2.35`) baseline; Windows builds target native `x86_64-pc-windows-msvc`.
- Windows builds are portable (unsigned, no installer).
- On GNOME Wayland, OS-level image clipboard interop with external apps is limited because GNOME Mutter does not implement `wlr-data-control`; in-app copy/paste works normally.
