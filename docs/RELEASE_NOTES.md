# Pyxross Workshop v0.9.2 Release Notes

## Downloads

Prebuilt portable binaries for **v0.9.2**:

- **Linux AppImage (x86_64, GLIBC 2.35+):** `pyxross-0.9.2-linux-x86_64.AppImage`
- **Linux Portable Archive (x86_64, GLIBC 2.35+):** `pyxross-0.9.2-linux-x86_64.tar.gz`
- **Windows Portable ZIP (x86_64, Windows 10/11):** `pyxross-0.9.2-windows-x86_64-portable.zip`

## What's New in 0.9.2

- **27 Layer Blend Modes:** Full W3C Compositing & Blending Level 1 support covering all 19 Aseprite blend modes (`0..=18`), Pyxel Edit blend modes, and standard artistic modes (`Darken`, `Multiply`, `Color Burn`, `Linear Burn`, `Darker Color`, `Lighten`, `Screen`, `Color Dodge`, `Add`, `Lighter Color`, `Overlay`, `Soft Light`, `Hard Light`, `Vivid Light`, `Linear Light`, `Pin Light`, `Hard Mix`, `Difference`, `Exclusion`, `Subtract`, `Divide`, `Invert`, `Hue`, `Saturation`, `Color`, `Luminosity`), with `.ase`/`.aseprite` and `.pyxel` import/export support.
- **Resize Canvas Undo/Redo:** Canvas count and region resizes are now fully undoable and redoable, restoring layer buffers, tilemaps, tile palette, tile size, and animation regions together.
- **Image-to-Pixel `Reload Previous` & Aspect-Locked Presets:** Automatically remembers the last used Image-to-Pixel settings even if not manually saved, with a `Reload Previous` button in the `Size` tab. When aspect lock is enabled, both `Reload Previous` and saved presets apply the preset width and compute height proportionally to the current source image.
- **`Back` vs `Cancel` in Import Flows:** Post-file-chooser dialogs (`Import Image`, `Import GIF`, `Import Aseprite`, `Import Pyxel`, `Image-to-Pixel`, `Import Banner`) now separate `Back` (return to file selection) from `Cancel` (close dialog).
- **Workspace Crosshair Cursor:** Hides the OS cursor over the canvas workspace (outside panels, popups, and gizmos) and renders a thin, subtle theme-tinted crosshair.
- **Tile Tool `Tab` Index Toggle:** Pressing `Tab` in the Tile tool toggles canvas tile ID labels on and off.
- **Fill & Wand `Contiguous (Alt)` XOR Control:** Both Fill and Fieldier Wand have a clickable `Contiguous (Alt)` checkbox; holding `Alt` temporarily inverts the current checked state and releasing restores it.

## Fixes

- **Tile Tool Drag Interpolation:** Fast left-click drags now interpolate across grid cells without gaps while preserving mid-hold `R`/`Q`/`X`/`Z` orientation changes and eliminating 1-frame preview lag.
- **Large Selection Performance:** `O(W+H)` rectangular outline generation, `Shape::closed_line` backing with viewport-clipped screen-space dashes, zero per-frame overlay cloning, and in-place outline translation during area moves.
- **UI Interaction Deselect Guard:** Interacting with panels, dock chrome, floating borders, top-bar menus, or popups no longer deselects active canvas selections.
- **Image-to-Pixel Single-Piece Import:** Importing as a new document now initializes a single full-canvas region instead of stale `16×16` 4-frame animations.
- **Animation Borders Alongside Selection:** Replaced yellow marching ants with clean static grey area + red first-frame + green last-frame borders that stay visible in parallel with active selections.
- **Resize Canvas Region–Pixel Binding:** Whole-region pivot offsets keep pixels, tilemap cells, tile palette entries, and animation frame regions locked together.

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
