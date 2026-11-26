# Changelog

All notable changes to **Pyxross Workshop** are documented in this file.

## [0.9.4] - 2026-11-26

### Added
- **Pencil & Eraser Canvas-Locked `Pattern` Mode:** added a `Pattern` selector (`Normal`, `Bayer Dither`, `Line`) in the Tool Property panel for both Pencil and Eraser (stored independently per tool), evaluated in absolute canvas coordinates `(x, y)` so symmetry and alignment remain unbroken across separate strokes, `Shift+Line` gestures, and Curve transforms:
  - **Bayer Dither:** 8×8 ordered Bayer matrix dithering with a configurable `Density` (`0..=100%`) slider (`50%` yields an exact 1×1 checkerboard).
  - **Line:** parallel stripes with configurable `Thickness` (`1..=32` px), `Angle` (`0..=180°`), and `Spacing` (`1..=64` px) sliders.
- **Scrollable Tool Property Panel:** wrapped Tool Property contents in a vertical scroll area so expanded pattern sliders remain accessible at any panel height.

### Fixed
- **Large Brush Line Performance:** eliminated lag when drawing lines with thick/large brushes (`32–64` px) in Pencil and Eraser by replacing per-stamp heap allocations and `BTreeSet` preview tracking with stack-allocated `BrushRowSpans`, delta leading-edge stepping along Bresenham lines, a word-level `FootprintMask` bitmask, merged horizontal-span `egui::Mesh` rendering (with cached nearest-neighbor texture fallback for dense pattern previews), and collinear-merged Eraser outlines.

## [0.9.3] - 2026-11-25

### Added
- **Brush Selector (`B`):** new selection child tool with configurable `Size` (`1..=64`, `Shift+Scroll`, `[`/`]`) and `Shape` (`Square` / `Circle`, `Ctrl+Scroll`); painting selects non-transparent (`alpha > 0`) pixels covered by the brush footprint, holding `Shift` adds to the selection, and right-clicking subtracts from the selection.
- **Categorized Toolbox:** reorganized the Toolbox into labeled **Draw** (Pencil, Eraser, Fill, Eyedropper), **Selection** (Rectangle, Wand, Lasso, Brush Selector), and **Misc** (Tile, Animation, Text) sections separated by subtle hairline dividers, exposing all selection tools directly and removing the parent Fieldier button.
- **File Manager Arrow Navigation:** navigate file/folder rows with `Up`/`Down` arrow keys (auto-scrolling the selected row into view), enter folders with `Right`/`Enter`, and go up a directory with `Left`/`Backspace`.
- **Layer Name Double-Click Settings:** double-clicking a layer's name in the Layers panel automatically opens (or focuses) its Layer Settings panel.
- **Onion Skin Behind & Layer Mode Controls:** added two new NerdFont toggle buttons flanking the Frame Editor's onion skin controls — **Onion Behind** (left) renders onion ghosts behind the current frame image, and **Layer Mode** (right) limits onion skinning to the active layer only.

### Fixed
- **Transform Commit Undo Pixel Loss:** masked transform cuts now record their undo delta from the live layer buffer via `DeltaRecorder`, preventing unselected pixels inside the transform bounding box from being erased on Undo.
- **Rectangle Double-Click & Shift-Select Ergonomics:** suppressed Rectangle double-click region selection when the first click commits a transform or dismisses a selection, added `Shift+Double-Click` to add regions to the current selection, and allowed `Shift`-selecting with any Fieldier tool when no selection exists to start a normal initial selection.
- **GIF Animation Export:** set `DisposalMethod::Background` on every exported GIF frame, added lossless exact local palettes for pixel-art frames (`<= 255` opaque colors with transparency), and applied each sequence's effective frame delays (`effective_delay_ms`) in Export Animation so frames no longer stack, ghost, or lose timing.
- **Tile Tool Layer Isolation:** right-click picking an empty cell now samples only the active layer's buffer instead of compositing all layers together into a single-layer stamp.
- **Tile Tool Placement Preview:** replaced per-pixel feathered rectangles with a cached nearest-neighbor preview texture quad so large tile previews render crisply without shattered grid seams.

## [0.9.2] - 2026-11-25

### Added
- **27 Layer Blend Modes:** expanded layer compositing with full W3C Compositing & Blending Level 1 math covering all 19 Aseprite blend modes (`0..=18`), Pyxel Edit blend modes, and standard digital art modes (`Darken`, `Multiply`, `Color Burn`, `Linear Burn`, `Darker Color`, `Lighten`, `Screen`, `Color Dodge`, `Add`, `Lighter Color`, `Overlay`, `Soft Light`, `Hard Light`, `Vivid Light`, `Linear Light`, `Pin Light`, `Hard Mix`, `Difference`, `Exclusion`, `Subtract`, `Divide`, `Invert`, `Hue`, `Saturation`, `Color`, `Luminosity`), including `.ase`/`.aseprite` and `.pyxel` import/export mappings.
- **Resize Canvas Undo/Redo:** canvas count and region resize operations now push a full reversible `ResizeCanvasCommand` onto the undo/redo stack, restoring layers, tilemaps, tile palette, tile size, and animation regions losslessly.
- **Image-to-Pixel Previous Settings & Aspect-Locked Presets:** automatically persists the last used Image-to-Pixel settings (`img2pixel_last_preset.json`) with a `Reload Previous` button in the `Size` tab; when `Lock` (aspect ratio) is enabled, both `Reload Previous` and saved presets preserve `target_width` and recalculate `target_height` to match the current image's aspect ratio.
- **Import Flow `Back` vs `Cancel`:** post-file-selection dialogs (`Import Image`, `Import GIF`, `Import Aseprite`, `Import Pyxel`, `Image-to-Pixel`, `Import Banner`) now provide a separate `Back` button to return to file selection and `Cancel` to close the dialog.
- **Workspace Custom Crosshair Cursor:** hides the OS pointer over the canvas workspace (when not over panels, popups, or transform gizmos) and draws a thin, subtle theme-tinted crosshair.
- **Tile Tool `Tab` Index Toggle:** pressing `Tab` while using the Tile tool toggles visibility of canvas tile ID overlays on and off.
- **Fill & Wand `Contiguous (Alt)` XOR Control:** both Fill and Fieldier Wand now provide a clickable `Contiguous (Alt)` checkbox whose active state is temporarily inverted while `Alt` is held and restored on release.

### Fixed
- **Tile Tool Left-Click Drag:** interpolated dragged cells along a Bresenham grid line so fast drags no longer leave gaps, preserved mid-hold `R`/`Q`/`X`/`Z` orientation changes, and eliminated 1-frame preview lag.
- **Large Selection Performance:** added an `O(W+H)` perimeter fast path for rectangular selections, switched marching-ants backing to `Shape::closed_line` with viewport-clipped screen-space dashes, removed per-frame overlay cloning, and replaced per-drag `capture_mask` copies with in-place segment translation.
- **Selection Preservation on UI Interaction:** clicking panels, dock chrome/splitters, floating borders, top-bar dropdowns, or popups outside the workspace no longer triggers selection deselect.
- **Image-to-Pixel Single-Piece Import:** importing as a new document now rebuilds the default animation set for the full image size instead of retaining stale `16×16` 4-frame regions.
- **Selected Animation Border & Parallel Selection View:** replaced yellow marching ants with a clean static grey outer area border plus red first-frame and green last-frame borders, and kept animation borders visible in parallel while a selection is active.
- **Resize Canvas Region–Pixel Binding:** `ResizeCounts` now shifts pixels, tilemap cells, and animation frame rects together by whole-region pivot multiples, and `ResizeRegions` resizes `TilePalette` entries in lockstep using ceiling region counts.

## [0.9.1] - 2026-11-22

### Added
- **File Manager:** sortable `Name`, `Size`, and `Modified` columns (ascending/descending), live autocomplete search in `File name` (including extension matching like `.png` with `Tab` completion), and a `File type` filter dropdown.
- **Image-to-Pixel:** smooth pointer-centered preview zoom & pan with `Fit` reset, plus `Save Preset` and `Load Preset` (`.json`) controls in the `Size` tab.
- **Import Banner Panel:** new two-column banner importer launched from `Select Banner` with `Banner Name` input, `Cut (8:1)` / `Full Image` modes, zoomable/pannable preview, draggable 8:1 crop frame with shaded exterior, and direct PNG export into `banners/`.
- **Top-Bar Banner Shortcut:** invisible clickable hotspot on the top-bar banner region with a subtle `"Click to change banner"` hover hint in the top-right corner.
- **Style Panel:** ergonomic `Save` (enabled when customized) and `Load` theme dialog workflow and a `Spacing` slider.
- **Key Bindings Panel:** redesigned with live search, category filter tabs (`Tools`, `Edit`, `Transform`, `File`, `Import / Export`, `View`), click-to-record monospace keycap badges, inline conflict indicators, manual key/modifier popover, and a pinned `Reset` / `Apply` footer.

### Fixed
- **Windows Input & Render Latency:** configured swapchain `desired_maximum_frame_latency = 1`, added `window.pre_present_notify()`, and flushed canvas texture uploads in the same frame after input handling to eliminate multi-frame cursor lag.
- **Overlapping Panel Z-Order:** clicking the front floating panel (or foreground popups) no longer raises occluded background panels or triggers background header drags.
- **Fast Canvas Edge Strokes:** rapid inward and outward brush swipes now interpolate cleanly across canvas borders without missing edge pixels while staying paused during off-canvas motion.
- **Panel Layout & Scrollbars:** fixed narrow-panel slider/button overlap in `Style`, pinned top action buttons in resizable `Select Banner`, eliminated resize scrollbar flicker and text overflow in `File Manager`, and kept `Key Bindings` footer buttons clear of scrollbars.
- **Pencil Default Shortcut:** guarded test keymap persistence so unit tests cannot overwrite user `keybindings.json` (keeping Pencil on `D`).

## [0.9.0] - 2026-11-20

### Added
- Dockable and floating panel workspace (`egui_dock`) with top bar, menus, and customizable pixel-art banners.
- Single-file `.pyxross` ZIP archive format (alongside directory projects) with autosave and crash recovery.
- **Aseprite** (`.ase` / `.aseprite`) and **Pyxel Edit** (`.pyxel`) import and export.
- Animation dock suite: multiple animations per project, live Playback panel, Frame Editor with onion skinning, and sprite sheet / GIF export.
- **Image-to-Pixel** converter (sampling, K-means palette extraction, dithering), inspired by `SLK_img2pixel`.
- Built-in cross-platform **File Manager** with OS standard folders, Windows drive enumeration, and Linux GTK2/3 + KDE Dolphin bookmarks.
- Pixel-perfect rotation and transform algorithms (**RotSprite**, **CleanEdge**, **Rotxel**) plus exact 90°/180°/270° and flip operations.
- Cross-platform Linux (`GLIBC 2.35` portable AppImage & `.tar.gz`) and Windows (`x86_64-msvc` portable `.zip`) automated release packaging.

### Improved
- GPU & swapchain resilience: `Outdated`/`Lost` surface reconfiguration, Windows minimize (`0x0`) guard, software adapter fallback (WARP / llvmpipe), native window icon, and Linux Wayland `app_id` / X11 `WM_CLASS`.
- Safe two-phase backup file replacement on Windows when saving `.pyxross` projects.
- Embedded **JetBrains Mono Nerd Font** with full SIL OFL 1.1 attribution.
