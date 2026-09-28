# Persistence

> **Status:** D08 implemented (R6 F1+F2, 612 tests green) — directory format + version migration + round-trip (`src/core/document.rs`, `src/io.rs`) and File menu wiring (New/Open/Save/Save As, dirty tracking, unsaved-changes guard). D16 autosave ✔ (R6 F3), D17 recovery journal ✔ (R6 F4) — 5-min autosave to `pyxross/autosave/<project>/` in the config dir, `recovery.json` journal, restore/discard modal at launch (625 tests green).

## 1. .pyxross Format [D08]

**Directory format (primary and only save format):**

```
MyProject.pyxross/
├── manifest.json
└── layers/
    └── <layer_id>/
        └── <frame_or_page>.png
```

- **manifest.json:** format version, canvas size, tile size, layer list (id, name, order, visibility, opacity, blend), regions/tiles (rect refs), frames (region ref + delay_ms), animation sequences/tags, palette, editor settings.
- Project palettes are stored in the manifest palette field. Application keybindings are stored separately at the platform config path `pyxross/keybindings.json`; user palettes use `pyxross/palettes/*.json`.
- Palette JSON files placed under a project `palettes/` directory are loaded when the project opens; the manifest palette remains the active project palette.
- Pixel data: per-layer PNGs. Layered data requires more than one flattened PNG — the directory layout supports 4096+ canvases and partial/streaming saves.
- Frame/tile pixel content is **not** duplicated: they are rect references into layer buffers; only layer pixels are stored.

### Round-trip requirement
save → load → compare must be lossless (unit-tested in core).

## 2. Autosave [D16]

- Interval: **5 minutes** default (configurable) → writes to user config dir `autosave/<project>/`.
- Never writes over the user's file directly; autosave is a separate copy.

## 3. Crash Recovery

- A small **recovery journal** tracks save state; on launch, if a recovery exists, the user is prompted: restore or discard.
- Recovery restores: last autosave + journal metadata.

## 4. Export Suite [D21]

| Target | Output |
|---|---|
| Full canvas | single PNG |
| Per-tile | one PNG per tile region |
| Sprite sheet | atlas PNG + JSON metadata (rects, frame durations, tags) |
| Animation | GIF |
| **Export Selected** | active region → PNG, **composite of all visible layers** (visibility/opacity/blend respected) [MVP must-have] |

## 5. Import [D20]

- v1: PNG → new canvas or new region source.
- Post-MVP order: Aseprite (`.ase/.aseprite`) → Pyxel Edit (`.pyxel`); import maps layers/cels/frames/tags/palette onto our model.
- OS clipboard PNG import also lands pixels (D14).

## 6. Versioning & Migration

- `manifest.json` carries `format_version`; loaders migrate older versions forward.
- Binary single-file export format: deferred decision (distribution option after v1).
