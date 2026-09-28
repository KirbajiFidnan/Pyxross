# UI / UX

> **Status:** Design targets only — nothing implemented yet (Rust rewrite, R0+).

## 1. App Flow

- **Startup [D31]:** editor opens directly, with an empty default document. No launcher screen, no startup dialog.
- New/Open/Save live in the File menu. The **New dialog** asks canvas size + tile size (default preset assumption: 256×256 canvas, 16×16 tiles — confirm in R6).
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