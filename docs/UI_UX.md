# UI / UX

> **Status:** Design targets only — nothing implemented yet (Rust rewrite, R0+).

## 1. App Flow

- **Startup [D31]:** editor opens directly, with an empty default document. No launcher screen, no startup dialog.
- New/Open/Save live in the File menu. The **New dialog** asks canvas size + tile size (default preset assumption: 256×256 canvas, 16×16 tiles — confirm in R6).
- **Save As [D82]:** always writes a single `.pyxross` file. The panel has **no** directory/single-file choice — only a static **`.pyxross`** format label. In-place **Ctrl+S** is unchanged (keeps the current path); only Save As is affected.
- Closing with unsaved changes → save prompt. Crash on next launch → recovery offer (see PERSISTENCE.md).

## 2. Default Layout (Pyxel Edit-faithful)

```
┌─────────┬──────────────────────┬───────────┐
│ Toolbox │                      │ Layers    │
│ (tools) │      CANVAS          │ Frame list│
│ Color/  │   (center, clipped)  │           │
│ Palette │                      │           │
├─────────┴──────────────────────┴───────────┤
│ Timeline · Playback controls               │
└─────────────────────────────────────────────┘
```

- Left: toolbox + color/palette. Right: layers + frame list. Bottom: timeline + playback. Center: canvas.
- The canvas widget is embedded in a clipped center region and never overpaints panels.
- **Fixed layout in v1 [D64]:** panel separators are draggable (sizes resizable), but panels do not float or detach. Dock/float system deferred post-MVP.

## 3. Panel System [D64]

- Fixed three-region layout: left (tools + palette), right (layers + frame list), bottom (timeline + playback). Center = canvas (never moves).
- Panel resizing via draggable separators.
- Deferred (post-MVP): dock↔float round-trip, multi-monitor placement, drag-to-redock, layout persistence (D60–D63 design retained as reference).

## 4. 9-Slice Skin System [D10, D11]

- Every skinnable component renders from pixel-art 9-slice sources with state variants (normal/hover/pressed/disabled).
- Theme = PNG atlas + JSON definition. Built-in default theme ships; user themes load from user config dir with **user-wins precedence**.
- Framework-independent custom rendering (egui painter / custom widget draw); a code bridge adapts built-in widgets where needed.
- **No hard-coded UI colors** anywhere in chrome (CONTEXT hard rule 3). A headless invariant test enforces this once implemented.
- Theme switch is atomic; malformed themes fall back cleanly (validate + fallback chain).
- Theme browser with refresh (⟳) for newly installed user themes + live preview.

## 5. Interaction Conventions

- **Canvas first:** tools operate directly on canvas; no modal dialogs for transforms [D06].
- Transform object gestures: see FEATURES.md §3.2 (Ctrl+LMB enter, body/corner/rotate/flip anatomy, Shift=15° snap, Esc cancel).
- Zoom: wheel zoom with integer snapping 10%–3200% [D28]; pan with middle/space-drag (keybinding map finalized in R8).
- Grid overlay always visible by default, toggle key in R8. [D41]
- Cursor feedback per tool (brush outline, fill cell highlight, transform handles) — drawn in render layer, themed.

## 6. UI Construction Rule [D30]

- All panels and widgets are **built in code** (egui); no separate UI definition files.
- Reasons: immediate-mode simplicity, skin-system integration, diff-friendly iteration.

## 7. File Manager [D81, D82]

Native/portal file dialogs are removed (D81): the **File Manager** panel (id 208) is the only picker for files and folders. It is presented as a **floating window titled "File Manager"**.

- **One reusable panel, three modes.** The mode sets the list filter and the OK label:
  - **Open Project** — `.pyxross` (directory or single-file, D80) + `manifest.json` + directories.
  - **Import PNG** — `*.png` + directories.
  - **Choose Folder** — directories only.
- **Top toolbar:** back ◀ / forward ▶ (disabled when there is no history in that direction); **Up** (disabled at the filesystem root); **New folder "+"**; an editable current-path field with a **▾ dropdown** that jumps to a parent directory.
- **List area:** a scrollable **well** that is **darker than the rest of the panel** (`list_bg`) and grows/shrinks with the panel. Rows use a **light/dark zebra** (`list_stripe_bg`) and look like the Layers-panel rows — **not buttons**. **Folders expand/collapse in place** with a `+`/`-` twisty (accordion, nestable), with **files listed last**. A single click selects; a double-click on a folder navigates into it; a double-click on a file confirms.
- **Bottom row:** a **"File name:"** editable field with a quick-pick **▾**; a **"File type:"** dropdown (defaults to the mode's filter — **"All formats" only widens the list**, while confirmation is still validated against the mode's filter); and **OK / Cancel**.
- **Keyboard:** Enter confirms, Esc cancels.
- **Chrome:** every color comes from theme tokens — no hard-coded colors (CONTEXT hard rule 3). Two tokens back the list: **`list_bg`** (the darker well) and **`list_stripe_bg`** (the zebra stripe); both have a built-in default and a `serde` default, and the shipped `theme.json` is unchanged. The panel is drawn through the existing panel system (float-only, static nest).
- **Save As integration:** the Save As panel's **"Change…"** opens the File Manager in **ChooseFolder** mode; the chosen folder is written back as the Save As destination.

## 8. Import Panels [D83, D84]

Both import panels share the base behavior of the Save As (id 207) and Create Document panels: **float-only**, **`NestMode::Static`**, and **not resizable** — they are moved only by dragging, never detached or resized. Their files arrive from the **File Manager** (Import GIF → `.gif` filter; Import Images → `.png` filter).

### Import GIF [D83]

- **Menu:** File → **"Import GIF…"** opens the File Manager filtered to `.gif`; the chosen file opens the **"Import GIF"** panel (id 209).
- **Content:** a **"Line length"** input (how many regions sit side by side per row). The frames are tiled left-to-right, row by row; when the frame count does not fill the last row, the surplus cells stay **empty** (e.g. 22 frames at line length 4 → a 4×6 grid with 2 empty cells).
- Import does not modify the current document; the content opens as a **new document** at its own size, named after the file's stem ("Untitled" if none).
- **OK / Cancel** placed **symmetrically** near the bottom center.

### Import Image [D84]

- **Menu:** File → **"Import Images…"** opens the File Manager filtered to `.png`; the chosen file opens the **"Import Image"** panel (id 210).
- **Content:** two **live-linked** inputs — **"Split by pixels"** (region size, W×H) and **"Split by number of regions"** (grid count, W×H) — laid out **symmetrically** about the center. Editing one updates the other immediately (two-way, `ceil`).
- When the split does not divide evenly, the content size rounds up to the nearest divisible number and opens as a **new document**.
- Import does not modify the current document; the content opens as a **new document** at its own size, named after the file's stem ("Untitled" if none).
- **OK / Cancel** placed **symmetrically** near the bottom center.

Both panels color strictly from theme tokens (no hard-coded colors — CONTEXT hard rule 3).