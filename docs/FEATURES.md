# Feature Specifications

> **Status:** Technical reset (2026-09-10, D64). Previous Godot implementation abandoned; all sections below are **design targets** for the Rust rewrite. Milestone references use the new R-series (see ROADMAP.md).
> MVP = Must-Have list in CONTEXT.md §5.

## 1. Tools (MVP) [D12, D38, D39, D75, D76, D77, D78]

| Tool | Behavior | Status |
|---|---|---|
| Pencil | Pixel brush; sizes 1×1/2×2/3×3/4×4, square/round variants; Bresenham-interpolated stamps | ⬜ |
| Eraser | Same brush model, erases to transparent | ⬜ |
| Flood fill | Contiguous + replace-all modes, shared tolerance (default 0) | ⬜ |
| Fieldier (select) | One tool with Rectangle / Wand / Lasso children (S / W / L, property-panel picker); drag marquee, magic-wand flood (Alt drops contiguity; tolerance slider + Ctrl+scroll), freehand lasso (self-intersections union), **Alt+drag = grid-cell region sweep** (Alt+left adds, Alt+right removes every grid cell the pointer passes over; Ctrl+drag no longer starts it); the committed selection is always drawn as calmer marching-ants over its mask (grey under-stroke, 3 px dashes, slower drift; a rectangle looks the same as a T-shape), **an area-move only translates the selection (rect + mask), never moves pixels and never pushes an undo step** (pixels move only via the transform object, D32), the live drag box takes the inverse of the canvas pixel under the pointer; **a Shift-add / subtract gesture keeps the base selection's marching-ants on screen** while the new area is swept; **Ctrl+click inside a selection lifts it as a cut, Alt+click no longer lifts a copy**; **Subtract = right mouse button only** (Alt is not Subtract), the right button never deselects and with no current selection does nothing; **each gesture's combining mode is locked at press** (not re-derived at release), and for **every child** the right button applies the same shape the left button would combined as `Subtract` (Rectangle / Wand / Lasso / Alt region sweep), while the left button adds with Shift and replaces otherwise; **the Wand applies its shape once per click** (primary = Shift-add / replace, secondary = subtract), not once per frame, and a **Wand double-click combines with the modifiers**: Shift extends the existing selection, the right button subtracts, and neither modifier replaces; **Alt+F rotates the selection 90° clockwise around its bounding-box center as one undo step (mask + pixels), and Shift+F / Ctrl+F flip the selection vertically / horizontally (each one undo step, mask + pixels)** [D75, D76, D77, D78, D79] | ⬜ |
| Move | Moves selection content; preview overlay + one commit Command | ⬜ |
| Transform (RotSprite) | See §3 — the signature feature | ⬜ |
| Flip H/V | Applies to selection (also available inside transform object) | ⬜ |
| Eyedropper | Reads **composite of all visible layers** (WYSIWYG); right-click temporary eyedropper returns to previous tool | ⬜ |

- **Clipboard:** internal region clipboard + OS clipboard (PNG both ways). Internal + PNG encode/decode in core; OS bridge later. [D14, D51]
- **Undo:** every user operation is a Command — reverse-delta (region + before/after bytes), unbounded stack, composite commands, one Command per user gesture [D13, D45]. **Structural ops are Commands too** [D13]: LayerAdd/LayerRemove/LayerReorder/LayerProperty travel the same UndoStack and never touch pixels — undo/redo restore the layer array by object identity [D59].

## 2. Tile & Region Model [D05, D37]

- Tiles/frames are rect references into the layered canvas — never pixel copies.
- **Stamp-ready model:** region data structure is designed with optional instancing from day one (so the save format never needs migration), but MVP placement behavior is plain stamping (pixel duplication).
- **Linked tiles** (edit source → all placed instances update live): post-MVP, enabled by the same data model.
- Tile grid overlay is visible by default at pixel-inspection zoom, and the configurable toggle suppresses it without changing pixels [D41].

## 3. Transform Object — RotSprite In-Canvas [D06, D32–D36]

**Principle (user requirement):** rotation/transform NEVER opens a dialog, window, or separate workspace. It happens directly on the canvas, Aseprite-style.

### 3.1 Lifecycle
1. **Enter:** make a selection, press **Ctrl+LMB** (or dedicated keybinding) → pixels lift off into a floating **transform object** drawn layer-less above all layers. Source pixels removed from the layer(s) — object owns them now.
2. **Manipulate:** drag handles (see 3.2). All math is live on screen.
3. **Exit:** commit (click outside or **Enter**) → pixels written back; **Esc** cancels (restores original pixels exactly).

### 3.2 Interaction anatomy [D33]
| Gesture | Action |
|---|---|
| Drag body | Move |
| Drag corner handle | Nearest-neighbor scale |
| Drag corner beyond body edge | Rotate around pivot |
| Alt + corner | Flip (mirror) through that axis |
| Shift while rotating | **22.5° snap** (default is free 1° — inverted convention; amended from 15° on 2026-09-12 per user) |

### 3.3 Rendering the transform [D34]
- **Hybrid thresholded preview:** selection ≤64×64 → live **RotSprite** during drag (true result visible); larger selections → fast nearest-neighbor preview during drag, real RotSprite applied at commit.
- Committed result is **always** true RotSprite — structure-preserving, no blur, no anti-aliasing.

### 3.4 Data handling — mini-stack [D32]
- Object internally holds **per-layer buffers** (mini-stack), visually layer-less.
- **Single-layer** (default): commit writes into **whichever layer is active at commit time** → dragging across layers = free layer move.
- **Multi-layer** (opt-in): every populated layer under the selection lifts its own buffer; commit returns **each buffer to its source layer**. No flattening, zero data loss, no cross-layer move semantics.
- **Undo:** one enter→commit session = **one undo step** (composite delta). [D35]
- Pivot: selectable point (default: selection center); rotation snaps around it.

### 3.5 Exact transforms
90°/180°/270° rotations and H/V flips are pixel-exact paths (no interpolation), available via keybindings and inside the transform object.

## 4. Animation [D05, D17]

- **Data model:** region-referenced frames — `Region` (rect ref, never pixel copies) + `Frame` (region + delay_ms) + `AnimationSequence` (ordered frames + loop flag) [D52].
- **Playback core:** deterministic `AnimationController` — `advance(delta_ms)` driven by the editor loop, `frame_changed`/`playback_started/stopped` events; no timer [D53].
- Timeline panel: frame strip, per-frame duration editing, drag-to-reorder, loop toggle.
- Playback: real-time, per-frame durations, loop.
- Onion skin [D43]: previous 1 frame reddish tint + next 1 frame greenish tint; count (0..3) and colors configurable via `OnionConfig`.
- Frame tagging/naming (future-friendly).

## 5. Layers [D04, D07]

- Add/insert/remove/reorder/duplicate; visibility, opacity (0–1 premultiplier), blend modes (normal, multiply, screen, add — expand later). Shape mirrors Aseprite's layer/cel model for later import compatibility.
- **Core:** `Layer` = name + visibility + opacity + blend + PixelBuffer; `LayerStack` owns the ordered array + active-layer selection (by object identity, so reorders/removals never lose it) and emits one coarse `changed()` per mutation [D29, D58]. `LayerStackController` facade: every structural gesture pushes exactly one undoable Command [D59]. Duplication is a sparse `PixelBuffer::duplicate()` (allocated chunks only, byte-independent copy).
- **Compositing (D58):** `Compositing::composite/composite_region` take a layer slice; hidden layers are skipped; opacity premultiplies source alpha; each layer blends against the accumulated backdrop (Aseprite/SVG semantics — blend applies to color, not transparency); with opacity 1 + NORMAL the math is byte-identical to straight-alpha "over" [D44].
- Selection/transform scope: active layer by default [D36].
- **Panel UI:** layer list with active-row selection, visibility/opacity/blend/duplicate/remove controls, cross-layer reorder drag; every mutating gesture pushes exactly one undo step through the `LayerStackController` [D59].

## 6. Panels (fixed layout, v1) [D64]

- **Fixed layout:** left = toolbox + color/palette; right = layers + frame list; bottom = timeline + playback. Center = canvas (fixed, never floats) [D64].
- Panel separators draggable (size adjustable); **dock/float system deferred post-MVP** (D60–D63 retained as design reference).
- Toolbox ↔ canvas sync: tool selection driven through a public `select_tool()`; `tool_changed` fired on deliberate switches only (the right-click temporary eyedropper does not emit) [D62 semantics retained].

## 7. Tile Placement Workflow (MVP = stamping) [D37]

1. Define a region (tile-sized rect) → becomes a named region.
2. Stamp it: click on canvas duplicates the region's pixels (all layers of that region composited).
3. Post-MVP: instances become live references — editing the source region updates all placements.

## 8. Export & Import [D20, D21]

- Export: full canvas PNG · per-tile PNGs · sprite sheet (atlas + JSON metadata) · GIF (animation) · **Export Selected** (active region → PNG, composite of visible layers, respecting visibility/opacity/blend).
- Import (v1): PNG as canvas/region source. Importers later: Aseprite → Pyxel Edit.

## 9. Editor Essentials

- Zoom 10%–3200% with integer snapping [D28]; pan.
- Palettes: embedded (PICO-8, DB16/DB32, NES…) + user palettes + RGB/HSV picker; palettes are a selection tool only — no pixel storage impact (RGBA8). [D18]
- R8 palette assets include PICO-8 and DB16; user JSON palettes in the platform config directory override embedded names deterministically.
- R8 keybindings are typed actions with JSON defaults and user overrides; the grid toggle defaults on and is configurable.
- Autosave every 5 min + crash recovery journal [D16] — details in PERSISTENCE.md.
- Fully customizable shortcuts [D19] — defaults in `assets/` JSON, overrides in user config dir.

## 10. Projections — Detached Canvas Surfaces [D65, D66]

**Principle (user requirement):** the artist may detach **canvas-only** views into real compositor surfaces — on a second monitor (frame-edit, playback, preview) or as a clean OBS recording target. Projections show a view of the document; they are never a second document, and they never receive input in v1 (view-only; transport/controls stay in the main window).

### 10.1 Surface types (view sources)

| View | Content | Milestone |
|---|---|---|
| Preview | Editor canvas at current camera state | R1 |
| Playback | Animation playback (syncs to `current_index` via change notifications [D53]) | R3 |
| Frame-edit | A region's canvas + tile grid | R3 |

Camera modes per projection: **Fit** (uses `Camera::fit_zoom_percent` [D63-based math]) / **Follow** (canvas camera) / **Fixed** (locked zoom).

### 10.2 Interaction (v1)

- **Menu:** View → New Preview Surface / New Playback Surface / New Frame-Edit Surface; each opens on the primary monitor by default (Fit mode).
- **Monitor picker:** a per-projection dropdown lists available outputs (layer-shell outputs on Linux / winit monitors on Windows). Selecting moves the projection there — app-controlled where the backend allows; elsewhere the user drags it manually (honest degradation).
- **Capture Mode toggle** (per projection + a global "record everything" default): canvas only — no HUD, grid/guides/onion forced off — for OBS recording.
- **Close:** closing a projection destroys only the surface; the document/view state is untouched (D61 semantics).

### 10.3 Platform behavior (capability matrix — honest)

| | Hyprland / wlroots / KWin | GNOME (fallback) | Windows |
|---|---|---|---|
| Backend | LayerShellProjector (SCTK) | WinitProjector | WinitProjector |
| Monitor targeting | ✅ app-controlled | ❌ user drags | ✅ exact |
| Always-on-top (playback overlay) | ✅ | ❌ | ✅ |
| OBS recording | Output capture of the projection monitor (window capture lists xdg-toplevels only) | Window capture | Window capture |
| Capture video | Always cursor-free (portal excludes cursor) | Always cursor-free | Cursor-free unless pointer enters window |

### 10.4 Capture-mode specifics

- OBS on Wayland: make the Capturing projection **full-monitor** and record that output via "Screen Capture (PipeWire)". The interaction target / HUD are on other monitors — the recorded video contains only the canvas.
- Redraw pacing on projections is compositor-driven (`wl_surface.frame`); a projection on a monitor with a different refresh rate runs at that monitor's rate.

### 10.5 Rendering

- Single wgpu device; projections render the **shared canvas texture** via a fullscreen quad (nearest-neighbor, integer zoom `floor(monitor_size/canvas_size)` — pixel-perfect, no blur). No per-projection CPU compositing, no pixel copies.
- Theme system untouched: v1 projections are canvas-only (no chrome, no hard-coded UI colors). If a HUD (frame number, FPS) lands later, it must come from the theme system (CONTEXT hard rule 3).
