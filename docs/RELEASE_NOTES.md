# Pyxross Workshop v0.9.3 Release Notes

## Downloads

Prebuilt portable binaries for **v0.9.3**:

- **Linux AppImage (x86_64, GLIBC 2.35+):** `pyxross-0.9.3-linux-x86_64.AppImage`
- **Linux Portable Archive (x86_64, GLIBC 2.35+):** `pyxross-0.9.3-linux-x86_64.tar.gz`
- **Windows Portable ZIP (x86_64, Windows 10/11):** `pyxross-0.9.3-windows-x86_64-portable.zip`

## What's New in 0.9.3

- **Brush Selector (`B`):** New selection child tool with configurable `Size` (`1..=64`, `Shift+Scroll`, `[`/`]`) and `Shape` (`Square` / `Circle`, `Ctrl+Scroll`). Painting selects non-transparent (`alpha > 0`) pixels covered by the brush footprint; holding `Shift` adds to the selection and right-clicking subtracts from it.
- **Categorized Toolbox:** Reorganized the Toolbox into labeled **Draw** (Pencil, Eraser, Fill, Eyedropper), **Selection** (Rectangle, Magic Wand, Lasso, Brush Selector), and **Misc** (Tile, Animation, Text) sections separated by subtle hairline dividers, exposing all selection tools directly and removing the standalone parent Fieldier button.
- **Onion Skin `Onion Behind` & `Layer Mode` Controls:** Added two NerdFont toggle buttons flanking the Frame Editor's onion skin controls — **Onion Behind** (left) composites each layer's onion skin directly underneath that layer's own current-frame pixels (so opaque lower layers never hide upper layers' onion skin), and **Layer Mode** (right) limits onion skinning to the active layer only.
- **File Manager Arrow Navigation:** Navigate file and folder rows using `Up`/`Down` arrow keys with automatic scroll-into-view, enter folders with `Right` or `Enter`, and move up a directory with `Left` or `Backspace`.
- **Layer Name Double-Click Settings:** Double-clicking a layer's name in the Layers panel automatically opens (or focuses) its Layer Settings panel.

## Fixes

- **GIF Animation Export:** Set `DisposalMethod::Background` on every exported GIF frame, added lossless exact local palettes for pixel-art frames (`<= 255` opaque colors with transparency), and applied each sequence's effective frame delays (`effective_delay_ms`) in Export Animation so frames no longer stack, ghost, or lose timing.
- **Tile Tool Layer Isolation:** Right-click picking an empty cell now samples only the active layer's buffer instead of compositing all layers together into a single-layer stamp.
- **Tile Tool Placement Preview:** Replaced per-pixel feathered rectangles with a cached nearest-neighbor preview texture quad so large tile previews render crisply without shattered grid seams.
- **Transform Commit Undo Pixel Loss:** Masked transform cuts now record their undo delta from the live layer buffer via `DeltaRecorder`, preventing unselected pixels inside the transform bounding box from being erased on Undo.
- **Rectangle Double-Click & Shift-Select Ergonomics:** Suppressed Rectangle double-click region selection when the first click commits a transform or dismisses a selection, added `Shift+Double-Click` to add regions to the current selection, and allowed `Shift`-selecting with any Fieldier tool when no selection exists to start a normal initial selection.

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
